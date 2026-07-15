//! Drains `DbResult` messages into the hierarchy, dialogs, and inspector —
//! a thin dispatcher over per-domain handler submodules; see ../AGENTS.md §db-results.

mod fields;
mod hierarchy;
mod nodes;
mod properties;
mod query;
mod roles;
mod terrain;
mod tokens;

use bevy::prelude::*;
use fe_runtime::messages::DbResult;

use super::VerseManager;
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
    mut primitive_cache: ResMut<super::PrimitiveDescriptorCache>,
) {
    for result in reader.read() {
        match result {
            DbResult::Seeded { .. } => hierarchy::handle_seeded(&db_sender),
            DbResult::HierarchyLoaded { verses } => hierarchy::handle_hierarchy_loaded(verses, &mut verse_mgr, &mut nav, &mut commands, &asset_server, &mut pending_api),
            DbResult::VerseJoined { .. } => hierarchy::handle_verse_joined(&db_sender),
            DbResult::DatabaseReset { .. } => {
                primitive_cache.clear();
                hierarchy::handle_database_reset(&mut verse_mgr, &db_sender);
            }
            DbResult::VerseCreated { id, name } => hierarchy::handle_verse_created(id, name, &mut verse_mgr),
            DbResult::FractalCreated { id, verse_id, name } => hierarchy::handle_fractal_created(id, verse_id, name, &mut verse_mgr),
            DbResult::PetalCreated { id, fractal_id, name } => hierarchy::handle_petal_created(id, fractal_id, name, &mut verse_mgr),
            DbResult::EntityRenamed { entity_type, entity_id, new_name } => hierarchy::handle_entity_renamed(entity_type, entity_id, new_name, &mut verse_mgr, &mut nav),
            DbResult::EntityDeleted { entity_type, entity_id } => hierarchy::handle_entity_deleted(entity_type, entity_id, &mut verse_mgr, &mut nav, &mut ui_mgr),
            DbResult::GltfImported { node_id, name, petal_id, asset_path, position, .. } => nodes::handle_gltf_imported(node_id, name, petal_id, asset_path, *position, &mut verse_mgr, &nav, &mut commands, &asset_server),
            DbResult::NodeCreated { id, petal_id, name, has_asset, correlation_id } => nodes::handle_node_created(id, petal_id, name, *has_asset, correlation_id.as_deref(), &mut verse_mgr, &nav, &mut ui_mgr, &mut path_state, &db_sender),
            DbResult::NodeDeleted { node_id, petal_id } => nodes::handle_node_deleted(node_id, petal_id, &mut verse_mgr, &nav, &mut path_state, &db_sender),
            DbResult::VerseInviteGenerated { invite_string, .. } => roles::handle_verse_invite_generated(invite_string, &mut ui_mgr),
            DbResult::PeerRolesResolved { scope, roles } => roles::handle_peer_roles_resolved(scope, roles, &mut ui_mgr),
            DbResult::RoleAssigned { peer_did, scope, role } => roles::handle_role_assigned(peer_did, scope, role, &mut ui_mgr),
            DbResult::RoleRevoked { peer_did, scope } => roles::handle_role_revoked(peer_did, scope, &mut ui_mgr),
            DbResult::ScopedInviteGenerated { invite_link } => roles::handle_scoped_invite_generated(invite_link, &mut ui_mgr),
            DbResult::LocalRoleResolved { scope, role } => roles::handle_local_role_resolved(scope, role, &mut local_role),
            DbResult::VerseDefaultAccessSet { verse_id, default_access } => roles::handle_verse_default_access_set(verse_id, default_access),
            DbResult::FractalDescriptionUpdated { fractal_id, description } => roles::handle_fractal_description_updated(fractal_id, description),
            DbResult::ApiTokenMinted { token, jti, scope, max_role, .. } => tokens::handle_api_token_minted(token, jti, scope, max_role, &mut ui_mgr, &mut inspector, &db_sender),
            DbResult::ApiTokenRevoked { jti } => tokens::handle_api_token_revoked(jti, revocation_tx.as_deref(), &inspector, &db_sender),
            DbResult::ApiTokensListed { tokens, total } => tokens::handle_api_tokens_listed(tokens, *total, &mut ui_mgr, &mut inspector),
            DbResult::ScopedApiTokensListed { tokens, total } => tokens::handle_scoped_api_tokens_listed(tokens, *total, &mut ui_mgr, &mut inspector),
            DbResult::QueryResult { data } => query::handle_query_result(data, &mut gis_panel, &mut path_state, &mut inspector),
            DbResult::Error(msg) => query::handle_error(msg, &mut gis_panel, &mut path_state, &mut inspector),
            // Property handlers return `false` to skip `try_deliver` for stale/unselected results.
            DbResult::NodePropertiesLoaded { node_id, properties } => { if !properties::handle_node_properties_loaded(node_id, properties, &mut path_state, &node_mgr, &mut inspector, &mut primitive_cache) { continue; } }
            DbResult::NodePropertySet { node_id, key } => { if !properties::handle_node_property_set(node_id, key, &nav, &db_sender, &mut path_state, &node_mgr, &mut inspector, &mut primitive_cache) { continue; } }
            DbResult::NodePropertyDeleted { node_id, key } => { if !properties::handle_node_property_deleted(node_id, key, &node_mgr, &mut inspector, &mut primitive_cache) { continue; } }
            DbResult::FieldDefsListed { field_defs, .. } => fields::handle_field_defs_listed(field_defs, &mut inspector),
            // Field-def mutations only need a re-list, which the panel re-sends itself.
            DbResult::FieldDefCreated { .. } | DbResult::FieldDefUpdated { .. } | DbResult::FieldDefDeleted { .. } => {}
            DbResult::PetalTerrainLoaded { petal_id, terrain } => terrain::handle_petal_terrain_loaded(petal_id, terrain, &nav, &mut petal_map),
            _ => {}
        }

        // Also try delivering every result to pending API requests.
        // This covers cases like ScopeResolved, NodeCreated, etc. that
        // the API thread may be waiting on.
        pending_api.try_deliver(result.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
    use super::*;
    use crate::actions::UiManager;
    use crate::dialogs::ActiveDialog;
    use crate::gis::{GisPanelState, PathEditorState};
    use crate::node_manager::NodeManager;
    use crate::plugin::InspectorFormState;
    use bevy::prelude::Entity;
    use fe_runtime::app::DbCommandSender;
    use fe_runtime::messages::{DbCommand, EntityType};
    use serde_json::json;

    fn sender() -> (DbCommandSender, crossbeam::channel::Receiver<DbCommand>) {
        let (tx, rx) = crossbeam::channel::bounded(8);
        (DbCommandSender(tx), rx)
    }

    fn tree() -> VerseManager {
        let mut mgr = VerseManager {
            verses: vec![VerseEntry {
                id: "v1".into(),
                name: "Verse".into(),
                namespace_id: None,
                expanded: true,
                fractals: vec![FractalEntry {
                    id: "f1".into(),
                    name: "Fractal".into(),
                    expanded: true,
                    petals: vec![PetalEntry {
                        id: "p1".into(),
                        name: "Petal".into(),
                        expanded: true,
                        nodes: vec![NodeEntry {
                            id: "n1".into(),
                            name: "Node".into(),
                            has_asset: false,
                            position: [0.0; 3],
                            webpage_url: None,
                            asset_path: None,
                        }],
                    }],
                }],
            }],
            ..Default::default()
        };
        mgr.rebuild_node_index();
        mgr
    }

    fn mgr_selected(node_id: &str) -> NodeManager {
        let mut mgr = NodeManager::default();
        mgr.select(Entity::from_bits(1), node_id);
        mgr
    }

    // --- properties::is_for_selected_node (selection guard) ---

    #[test]
    fn is_for_selected_node_true_when_matching() {
        assert!(properties::is_for_selected_node(&mgr_selected("node-1"), "node-1"));
    }

    #[test]
    fn is_for_selected_node_false_when_different_node() {
        assert!(!properties::is_for_selected_node(&mgr_selected("node-1"), "node-2"));
    }

    #[test]
    fn is_for_selected_node_false_when_nothing_selected() {
        assert!(!properties::is_for_selected_node(&NodeManager::default(), "node-1"));
    }

    // --- hierarchy handlers ---

    #[test]
    fn seeded_and_verse_joined_send_load_hierarchy() {
        let (tx, rx) = sender();
        hierarchy::handle_seeded(&tx);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
        hierarchy::handle_verse_joined(&tx);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
    }

    #[test]
    fn database_reset_clears_tree_and_reloads() {
        let (tx, rx) = sender();
        let mut mgr = tree();
        hierarchy::handle_database_reset(&mut mgr, &tx);
        assert!(mgr.verses.is_empty());
        assert!(mgr.node_index.is_empty());
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
    }

    #[test]
    fn verse_fractal_petal_created_grow_the_tree() {
        let mut mgr = tree();
        hierarchy::handle_verse_created("v2", "V2", &mut mgr);
        assert_eq!(mgr.verses.len(), 2);
        hierarchy::handle_fractal_created("f2", "v2", "F2", &mut mgr);
        assert_eq!(mgr.verses[1].fractals.len(), 1);
        hierarchy::handle_petal_created("p2", "f2", "P2", &mut mgr);
        assert_eq!(mgr.verses[1].fractals[0].petals.len(), 1);
    }

    #[test]
    fn entity_renamed_updates_tree_and_active_nav_name() {
        let mut mgr = tree();
        let mut nav = NavigationManager::default();
        nav.active_verse_id = Some("v1".into());
        hierarchy::handle_entity_renamed(&EntityType::Verse, "v1", "Renamed", &mut mgr, &mut nav);
        assert_eq!(mgr.verses[0].name, "Renamed");
        assert_eq!(nav.active_verse_name, "Renamed");
    }

    #[test]
    fn entity_deleted_prunes_rebuilds_index_and_closes_dialog() {
        let mut mgr = tree();
        let mut nav = NavigationManager::default();
        let mut ui = UiManager::default();
        ui.open_dialog(ActiveDialog::PeerDebug);
        hierarchy::handle_entity_deleted(&EntityType::Verse, "v1", &mut mgr, &mut nav, &mut ui);
        assert!(mgr.verses.is_empty());
        assert!(mgr.node_index.is_empty(), "index must be rebuilt after structural delete");
        assert!(matches!(ui.active_dialog, ActiveDialog::None));
    }

    // --- node handlers ---

    #[test]
    fn node_created_adds_node_and_indexes_it() {
        let (tx, rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default(); // no active petal → no Paths re-query
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        nodes::handle_node_created("n2", "p1", "New", false, None, &mut mgr, &nav, &mut ui, &mut path_state, &tx);
        assert!(mgr.all_nodes().any(|n| n.id == "n2"));
        assert!(mgr.node_index.contains_key("n2"));
        assert!(rx.try_recv().is_err(), "inactive petal must not re-query tracks");
    }

    #[test]
    fn node_created_in_active_petal_reruns_paths_query() {
        let (tx, rx) = sender();
        let mut mgr = tree();
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        nodes::handle_node_created("n2", "p1", "New", false, None, &mut mgr, &nav, &mut ui, &mut path_state, &tx);
        assert!(rx.try_recv().is_ok(), "active-petal create must re-run the Paths query");
    }

    #[test]
    fn node_deleted_prunes_node_and_stops_editing_it() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        path_state.editing_track_id = Some("n1".into());
        nodes::handle_node_deleted("n1", "p1", &mut mgr, &nav, &mut path_state, &tx);
        assert!(mgr.all_nodes().all(|n| n.id != "n1"));
        assert!(!mgr.node_index.contains_key("n1"));
        assert!(path_state.editing_track_id.is_none());
    }

    // --- roles / dialog handlers ---

    #[test]
    fn verse_invite_generated_opens_invite_dialog() {
        let mut ui = UiManager::default();
        roles::handle_verse_invite_generated("fractal://invite", &mut ui);
        match &ui.active_dialog {
            ActiveDialog::InviteDialog { invite_string, include_write_cap, expiry_hours } => {
                assert_eq!(invite_string, "fractal://invite");
                assert!(!include_write_cap);
                assert_eq!(*expiry_hours, 24);
            }
            other => panic!("expected InviteDialog, got {other:?}"),
        }
    }

    #[test]
    fn local_role_resolved_caches_level() {
        let mut local_role = crate::plugin::LocalUserRole::default();
        roles::handle_local_role_resolved("VERSE#v1", "editor", &mut local_role);
        assert_eq!(local_role.role, Some(fe_database::RoleLevel::Editor));
    }

    // --- query / error routing ---

    #[test]
    fn error_routes_by_claim_priority() {
        // gis panel first
        let (mut gis, mut path, mut ins) =
            (GisPanelState::default(), PathEditorState::default(), InspectorFormState::default());
        gis.query_pending = true;
        query::handle_error("boom", &mut gis, &mut path, &mut ins);
        assert_eq!(gis.last_error.as_deref(), Some("boom"));
        assert!(!gis.query_pending);
        // then paths tab
        let (mut gis, mut path, mut ins) =
            (GisPanelState::default(), PathEditorState::default(), InspectorFormState::default());
        path.tracks_pending = true;
        query::handle_error("boom", &mut gis, &mut path, &mut ins);
        assert_eq!(path.last_error.as_deref(), Some("boom"));
        // then ad-hoc query tab
        let (mut gis, mut path, mut ins) =
            (GisPanelState::default(), PathEditorState::default(), InspectorFormState::default());
        ins.query_loading = true;
        query::handle_error("boom", &mut gis, &mut path, &mut ins);
        assert_eq!(ins.query_result.as_deref(), Some("Error: boom"));
        assert!(!ins.query_loading);
    }

    #[test]
    fn query_result_falls_back_to_inspector_buffer() {
        let (mut gis, mut path, mut ins) =
            (GisPanelState::default(), PathEditorState::default(), InspectorFormState::default());
        ins.query_loading = true;
        query::handle_query_result(&[json!({"a": 1})], &mut gis, &mut path, &mut ins);
        assert!(ins.query_result.as_deref().is_some_and(|s| s.contains("\"a\": 1")));
        assert!(!ins.query_loading);
    }

    // --- property handlers (continue semantics) ---

    #[test]
    fn properties_loaded_skips_delivery_when_not_selected() {
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let delivered = properties::handle_node_properties_loaded(
            "n1", &json!({}), &mut path_state, &NodeManager::default(), &mut ins, &mut cache,
        );
        assert!(!delivered, "unselected result must continue (skip try_deliver)");
    }

    #[test]
    fn properties_loaded_feeds_primitive_cache_even_when_unselected() {
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let props = json!({"primitive": {"kind": "cube", "dims": [1.0, 1.0, 1.0]}});
        let delivered = properties::handle_node_properties_loaded(
            "n1", &props, &mut path_state, &NodeManager::default(), &mut ins, &mut cache,
        );
        assert!(!delivered, "selection gate unchanged");
        assert!(cache.get("n1").is_some(), "FR-1: cache fed without selection");
    }

    #[test]
    fn primitive_property_set_invalidates_cache_and_issues_readback_unselected() {
        let (tx, rx) = sender();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &json!({"primitive": {"kind": "sphere", "dims": [1.0]}}));
        let for_selected = properties::handle_node_property_set(
            "n1", "primitive", &nav, &tx, &mut path_state, &NodeManager::default(), &mut ins, &mut cache,
        );
        assert!(!for_selected);
        assert!(cache.get("n1").is_none(), "stale descriptor must be evicted");
        assert!(
            matches!(rx.try_recv(), Ok(DbCommand::GetNodeProperties { node_id }) if node_id == "n1"),
            "primitive write must re-fetch even when unselected"
        );
    }

    #[test]
    fn primitive_property_deleted_evicts_cache_even_when_unselected() {
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &json!({"primitive": {"kind": "sphere", "dims": [1.0]}}));
        let delivered = properties::handle_node_property_deleted(
            "n1", "primitive", &NodeManager::default(), &mut ins, &mut cache,
        );
        assert!(!delivered);
        assert!(cache.get("n1").is_none(), "delete must evict before the selection gate");
    }

    #[test]
    fn properties_loaded_populates_inspector_for_selected_node() {
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let props = json!({"gis.annotation.title": "T", "gis.annotation.body": "B"});
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let delivered = properties::handle_node_properties_loaded(
            "n1", &props, &mut path_state, &mgr_selected("n1"), &mut ins, &mut cache,
        );
        assert!(delivered);
        assert_eq!(ins.node_properties, props);
        assert!(!ins.node_properties_loading);
        assert_eq!(ins.annotation_title_buf, "T");
        assert_eq!(ins.annotation_body_buf, "B");
        assert_eq!(ins.annotation_color_buf, "");
    }

    #[test]
    fn property_set_issues_readback_for_selected_node() {
        let (tx, rx) = sender();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let for_selected = properties::handle_node_property_set(
            "n1", "some.key", &nav, &tx, &mut path_state, &mgr_selected("n1"), &mut ins, &mut cache,
        );
        assert!(for_selected);
        assert!(ins.node_properties_loading);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::GetNodeProperties { node_id }) if node_id == "n1"));
    }

    #[test]
    fn property_set_unselected_skips_readback_and_delivery() {
        let (tx, rx) = sender();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let for_selected = properties::handle_node_property_set(
            "n1", "some.key", &nav, &tx, &mut path_state, &NodeManager::default(), &mut ins, &mut cache,
        );
        assert!(!for_selected);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn property_deleted_clears_only_the_touched_buffer() {
        let mut ins = InspectorFormState::default();
        ins.node_properties = json!({"gis.annotation.title": "T", "gis.annotation.body": "B"});
        ins.annotation_title_buf = "T".into();
        ins.annotation_body_buf = "B".into();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let delivered = properties::handle_node_property_deleted(
            "n1", "gis.annotation.title", &mgr_selected("n1"), &mut ins, &mut cache,
        );
        assert!(delivered);
        assert!(ins.annotation_title_buf.is_empty());
        assert_eq!(ins.annotation_body_buf, "B", "sibling buffer must survive");
        assert!(ins.node_properties.get("gis.annotation.title").is_none());
    }

    // --- fields / terrain / tokens ---

    #[test]
    fn field_defs_listed_populates_inspector() {
        let mut ins = InspectorFormState::default();
        ins.field_defs_loading = true;
        let defs = vec![fe_runtime::messages::FieldDefInfo {
            field_def_id: "fd1".into(),
            scope: "VERSE#v1".into(),
            entity_type: "node".into(),
            key: "temp".into(),
            value_type: "number".into(),
            default_val: None,
            created_by: "did:x".into(),
            created_at: "now".into(),
        }];
        fields::handle_field_defs_listed(&defs, &mut ins);
        assert_eq!(ins.field_defs.len(), 1);
        assert_eq!(ins.field_defs[0].key, "temp");
        assert!(!ins.field_defs_loading);
    }

    #[test]
    fn petal_terrain_loaded_gates_on_active_petal() {
        let mut nav = NavigationManager::default();
        let mut map = crate::terrain_map::PetalMapState::default();
        let terrain = Some(json!({"world_scale": 2.5, "tileset_hexon_uris": ["hexon://a"]}));
        // Inactive petal → untouched.
        terrain::handle_petal_terrain_loaded("p1", &terrain, &nav, &mut map);
        assert!(!map.loaded);
        // Active petal → parsed.
        nav.active_petal_id = Some("p1".into());
        terrain::handle_petal_terrain_loaded("p1", &terrain, &nav, &mut map);
        assert!(map.loaded);
        assert_eq!(map.world_scale, 2.5);
        assert_eq!(map.tileset_ids, vec!["hexon://a".to_string()]);
    }

    #[test]
    fn api_tokens_listed_populates_inspector() {
        let mut ui = UiManager::default();
        let mut ins = InspectorFormState::default();
        ins.api_tokens_loading = true;
        let infos = vec![fe_runtime::messages::ApiTokenInfo {
            jti: "j1".into(),
            scope: "VERSE#v1".into(),
            max_role: "viewer".into(),
            label: Some("ci".into()),
            created_at: "t0".into(),
            expires_at: "t1".into(),
            revoked: false,
            sub: "did:me".into(),
        }];
        tokens::handle_api_tokens_listed(&infos, 1, &mut ui, &mut ins);
        assert_eq!(ins.api_tokens.len(), 1);
        assert_eq!(ins.api_tokens[0].jti, "j1");
        assert_eq!(ins.api_tokens_total, 1);
        assert!(!ins.api_tokens_loading);
    }
}
