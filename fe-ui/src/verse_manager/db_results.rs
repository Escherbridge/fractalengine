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
    mut gis_panel: ResMut<crate::gis::GisPanelState>,
    mut path_state: ResMut<crate::gis::PathEditorState>,
    node_mgr: Res<crate::node_manager::NodeManager>,
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

            DbResult::NodeCreated { id, petal_id, name, has_asset, .. } => {
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
                // FR-3: any node create in the active petal may be a track —
                // re-run the Paths tab query rather than relying on the
                // manual Refresh button as the only sync path.
                if nav.active_petal_id.as_deref() == Some(petal_id.as_str()) {
                    crate::actions::path::query_tracks(&db_sender, &mut path_state, petal_id.clone());
                }
            }

            // FR-1/FR-3: generic node-delete lifecycle event. Fixes the
            // left-panel orphan for all node deletes, not just tracks, and
            // re-runs the Paths tab query so a deleted track vanishes
            // without requiring the manual Refresh button.
            DbResult::NodeDeleted { node_id, petal_id } => {
                if let Some(petal) = verse_mgr.find_petal_mut(petal_id) {
                    petal.nodes.retain(|n| n.id != *node_id);
                }
                if path_state.editing_track_id.as_deref() == Some(node_id.as_str()) {
                    path_state.stop_editing();
                }
                if nav.active_petal_id.as_deref() == Some(petal_id.as_str()) {
                    crate::actions::path::query_tracks(&db_sender, &mut path_state, petal_id.clone());
                }
                bevy::log::info!("Deleted node {node_id} (petal {petal_id})");
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
                // GIS panel queries take priority over the ad-hoc Query tab
                // when both happen to be in flight — see `DbResult::QueryResult`
                // below for why these two share one untagged result channel.
                if gis_panel.query_pending {
                    gis_panel.last_error = Some(msg.clone());
                    gis_panel.query_pending = false;
                } else if path_state.tracks_pending {
                    path_state.last_error = Some(msg.clone());
                    path_state.tracks_pending = false;
                } else if inspector.query_loading {
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
                // `RawQuery`'s result carries no request-id to correlate against,
                // so whichever caller is actually waiting claims it: the GIS
                // panel's queries take priority since they're the newer/rarer
                // caller — the ad-hoc Query tab falls back to its own buffer.
                if gis_panel.query_pending {
                    gis_panel.results = crate::gis::parse_gis_rows(data);
                    gis_panel.query_pending = false;
                } else if path_state.tracks_pending {
                    crate::actions::path::apply_track_rows(&mut path_state, crate::gis::parse_gis_rows(data));
                } else {
                    let formatted = serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("Format error: {e}"));
                    inspector.query_result = Some(formatted);
                    inspector.query_loading = false;
                }
            }

            // --- Property value results ---
            // All three arms below are gated on `node_id` still matching the
            // current selection: `NodePropertySet`'s refetch-after-save is
            // async, so if the user has since selected a different node, a
            // stale response must not stomp that node's Properties/Annotation
            // buffers. See `fe-ui/src/AGENTS.md` §gis-query-ui
            // annotation-save-fix.
            DbResult::NodePropertiesLoaded { ref node_id, ref properties } => {
                // Path editor's `gpx_points` read-back (PathSelectTrack): a
                // DIFFERENT claimant from the inspector's `is_for_selected_node`
                // guard below — `NodePropertiesLoaded` has no correlation id
                // and is broadcast to every reader, so the inspector's own
                // GetNodeProperties reply must not stomp this buffer and vice
                // versa. Gated on `editing_track_id` still matching (guards a
                // stale reply after the user picked a different track or left
                // the editor) + `points_pending` (only the read-back we asked
                // for, not e.g. a later unrelated load). See AGENTS.md §path-editor.
                if path_state.editing_track_id.as_deref() == Some(node_id.as_str()) && path_state.points_pending {
                    path_state.points = properties
                        .get("gpx_points")
                        .map(crate::gis::decode_gpx_points)
                        .unwrap_or_default();
                    path_state.points_pending = false;
                }
                if !is_for_selected_node(&node_mgr, node_id) {
                    continue;
                }
                inspector.node_properties = properties.clone();
                inspector.node_properties_loading = false;
                // Annotation card buffers derive from the same properties object.
                let (title, body, color) =
                    crate::actions::node_props::annotation_fields_from_properties(properties);
                inspector.annotation_title_buf = title;
                inspector.annotation_body_buf = body;
                inspector.annotation_color_buf = color;
            }
            DbResult::NodePropertySet { ref node_id, ref key } => {
                // FR-3: a track-identity property landing anywhere (not just
                // the currently-selected node) means the Paths tab's track
                // list may now be stale — re-run the query unconditionally
                // for that one key, independent of the selection guard below.
                if key == "gis.track.name" {
                    if let Some(petal_id) = nav.active_petal_id.clone() {
                        crate::actions::path::query_tracks(&db_sender, &mut path_state, petal_id);
                    }
                }
                if !is_for_selected_node(&node_mgr, node_id) {
                    continue;
                }
                // Re-fetch properties for the node to refresh UI
                inspector.node_properties_loading = true;
                let _ = db_sender.0.send(DbCommand::GetNodeProperties {
                    node_id: node_id.clone(),
                });
            }
            DbResult::NodePropertyDeleted { ref node_id, ref key } => {
                if !is_for_selected_node(&node_mgr, node_id) {
                    continue;
                }
                // Remove the key locally for immediate UI feedback
                if let Some(obj) = inspector.node_properties.as_object_mut() {
                    obj.remove(key.as_str());
                }
                // Update only the buffer for the deleted key rather than
                // re-deriving all three from `inspector.node_properties` —
                // a single "Save Annotation" click emits one action per
                // field (Set for non-empty, Delete for empty), and that
                // cache may still be missing a sibling field's just-Set
                // value (its own NodePropertySet→refetch round-trip hasn't
                // resolved yet) when this Delete result lands. Re-deriving
                // from the stale cache would visibly revert the sibling
                // field to blank even though its write already landed in
                // the DB — this was the annotation-save bug.
                match crate::actions::node_props::annotation_field_for_key(key) {
                    Some(crate::actions::node_props::AnnotationField::Title) => {
                        inspector.annotation_title_buf.clear();
                    }
                    Some(crate::actions::node_props::AnnotationField::Body) => {
                        inspector.annotation_body_buf.clear();
                    }
                    Some(crate::actions::node_props::AnnotationField::Color) => {
                        inspector.annotation_color_buf.clear();
                    }
                    None => {}
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
                    // Restore the stored world scale (drives the settings slider + camera).
                    petal_map.world_scale = terrain
                        .as_ref()
                        .and_then(|t| t.get("world_scale"))
                        .and_then(|v| v.as_f64())
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .unwrap_or(1.0);
                    // Hexon-authoritative clamp bounds (scale orchestration track); see fe-ui/src/verse_manager/AGENTS.md.
                    petal_map.scale_bounds = terrain
                        .as_ref()
                        .and_then(|t| t.get("scale_bounds"))
                        .and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok());
                    // Keep the raw doc for the GIS Layer Manager's mutate-and-round-trip flow.
                    petal_map.terrain_json = terrain.clone();
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

/// `true` when `node_id` is still the currently-selected node — guards
/// async property-load/set/delete results against stomping a different
/// node's buffers after the user has switched selection mid-round-trip. See
/// `fe-ui/src/AGENTS.md` §gis-query-ui annotation-save-fix.
fn is_for_selected_node(node_mgr: &crate::node_manager::NodeManager, node_id: &str) -> bool {
    node_mgr.selected.as_ref().is_some_and(|s| s.node_id == node_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_manager::NodeManager;
    use bevy::prelude::Entity;

    fn mgr_selected(node_id: &str) -> NodeManager {
        let mut mgr = NodeManager::default();
        mgr.select(Entity::from_bits(1), node_id);
        mgr
    }

    #[test]
    fn is_for_selected_node_true_when_matching() {
        let mgr = mgr_selected("node-1");
        assert!(is_for_selected_node(&mgr, "node-1"));
    }

    #[test]
    fn is_for_selected_node_false_when_different_node() {
        let mgr = mgr_selected("node-1");
        assert!(!is_for_selected_node(&mgr, "node-2"));
    }

    #[test]
    fn is_for_selected_node_false_when_nothing_selected() {
        let mgr = NodeManager::default();
        assert!(!is_for_selected_node(&mgr, "node-1"));
    }
}
