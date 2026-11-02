//! Drains `DbResult` messages into the hierarchy, dialogs, and inspector —
//! a thin dispatcher over per-domain handler submodules; see ../AGENTS.md §db-results.

mod committed;
mod fields;
mod hierarchy;
mod nodes;
mod properties;
mod query;
mod roles;
mod terrain;
pub(crate) mod tokens;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use fe_runtime::messages::DbResult;

use super::VerseManager;
use crate::navigation_manager::NavigationManager;

/// The two fe-ui-local descriptor caches plus the path-asset applied-gate,
/// the T2 stamp interaction state, and the spawn-guard state, grouped as one
/// `SystemParam` so `apply_db_results` stays within Bevy's 16-tuple limit. The caches are fed from the
/// property handlers on `NodePropertiesLoaded`/`Set`/`Deleted`; `NodeDeleted`
/// invalidates `path_asset` + `applied` so a deleted track's stamps cascade
/// away (FR-4); `spawned`/`mesh_budget` gate `HierarchyLoaded`
/// re-materialization (see ../AGENTS.md §db-results).
#[derive(SystemParam)]
pub(super) struct DescriptorCaches<'w, 's> {
    committed_hierarchy_pending: Local<'s, bool>,
    primitive: ResMut<'w, super::PrimitiveDescriptorCache>,
    path_asset: ResMut<'w, super::PathAssetCache>,
    applied: ResMut<'w, super::PathAssetApplied>,
    /// T2 stamp bookkeeping: `NodePromoted` marks + flushes buffered overrides;
    /// `NodePropertiesLoaded` hydrates promoted stamps on reload (§stamped-assets).
    stamp_state: ResMut<'w, crate::actions::asset::StampInteractionState>,
    /// T3 earthwork bookkeeping: `NodeCreated` binds `earthwork:{region_id}`
    /// correlations + writes the endpoint property bag; `NodePropertiesLoaded`
    /// re-hydrates region→node on reload (§sculpt in actions/AGENTS.md).
    earthwork_map: ResMut<'w, crate::actions::terrain_proposal::EarthworkNodeMap>,
    /// HS-P6-CALLOUT bookkeeping: `NodeCreated` binds `callout:{id}`
    /// correlations + writes the endpoint property bag; `NodeDeleted` drops
    /// it; `NodePropertiesLoaded` re-hydrates node→geometry on reload
    /// (§dimension-callout in actions/AGENTS.md).
    callout_map: ResMut<'w, crate::actions::dimension_callout::DimensionCalloutMap>,
    /// Proposal-mirror rehydration target for `PetalTerrainLoaded` (data-loss
    /// fix: the mirror must be seeded from the doc before any embed persists).
    proposal_state: ResMut<'w, crate::terrain_proposal_state::ProposalEditState>,
    /// M2/F6: per-verse timeseries fabric settings surface state, fed by
    /// `HierarchyLoaded` + the `VerseTimeseriesSettingsSet` echo. Bundled here
    /// (not a 17th top-level param) — `apply_db_results` is already at Bevy's
    /// 16-`SystemParam` ceiling.
    ts_settings: ResMut<'w, crate::timeseries_settings::TimeseriesSettingsState>,
    spawned: Query<
        'w,
        's,
        &'static crate::plugin::SpawnedNodeMarker,
        Without<super::spawn::PathAssetInstance>,
    >,
    current_nodes: Query<
        'w,
        's,
        (
            Entity,
            &'static crate::plugin::SpawnedNodeMarker,
            &'static Transform,
        ),
        With<super::spawn::ResidentNode>,
    >,
    mesh_budget: Res<'w, crate::plugin::MeshInstanceBudget>,
    /// D-74: `HierarchyLoaded` only spawns ledger-admitted nodes (§residency-ledger).
    residency: Res<'w, super::ResidencyLedger>,
    /// Phase 3 visibility hydration target (`view.state`/`view.groups` via
    /// `NodePropertiesLoaded`, `PetalViewStateLoaded`, `NodeViewSet`).
    visibility: ResMut<'w, crate::visibility::VisibilityState>,
    /// Notice clock for every notice this system shows (`notice::NoticeClock`).
    time: Res<'w, crate::notice::NoticeClock>,
}

fn forward_correlated_property_reply(
    result: &DbResult,
    pending: &mut fe_runtime::app::PendingApiRequests,
) -> bool {
    let correlated = matches!(
        result,
        DbResult::NodePropertiesLoaded {
            correlation_id: Some(_),
            ..
        } | DbResult::NodePropertySet {
            correlation_id: Some(_),
            ..
        } | DbResult::NodePropertyDeleted {
            correlation_id: Some(_),
            ..
        }
    );
    if correlated {
        pending.try_deliver(result.clone());
    }
    correlated
}

fn request_committed_hierarchy(pending: &mut bool, nav: &mut NavigationManager) {
    nav.suppress_initial_auto_navigation();
    *pending = true;
}

fn flush_committed_hierarchy(sender: &fe_runtime::app::DbCommandSender, pending: &mut bool) {
    if !*pending {
        return;
    }
    match sender
        .0
        .try_send(fe_runtime::messages::DbCommand::LoadHierarchy)
    {
        Ok(()) => *pending = false,
        Err(crossbeam::channel::TrySendError::Full(_)) => {}
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            *pending = false;
            bevy::log::warn!("DB channel closed while hydrating committed object");
        }
    }
}

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
    mut node_mgr: ResMut<crate::node_manager::NodeManager>,
    mut caches: DescriptorCaches,
) {
    // Live scene node_ids: repeat HierarchyLoaded events (API polls, joins)
    // must not re-spawn them. Updated in-place so several HierarchyLoaded in
    // one batch can't double-spawn either (deferred spawns are query-invisible).
    let mut already_spawned: std::collections::HashSet<String> =
        caches.spawned.iter().map(|m| m.node_id.clone()).collect();
    let mesh_budget_exceeded = caches.mesh_budget.exceeded;
    for result in reader.read() {
        if matches!(result, DbResult::NodeUrlUpdated { .. }) {
            crate::actions::portal::finish_url_save(&mut ui_mgr, &mut verse_mgr, &nav, &node_mgr, result, caches.time.elapsed_secs_f64(), &db_sender.0, &inspector);
            continue;
        }
        if matches!(result, DbResult::NodeUrlRead { .. }) {
            crate::actions::url_read_model::finish_read(&mut ui_mgr, &mut verse_mgr, &nav, &node_mgr, &mut inspector, result, caches.time.elapsed_secs_f64(), &db_sender.0);
            continue;
        }
        crate::object_data::synchronize(
            &mut inspector.object_data,
            &node_mgr,
            &verse_mgr,
            &nav,
            &local_role,
        );
        if crate::object_data::route(&mut inspector.object_data, result) {
            continue;
        }
        crate::actions::asset::handle_edit_reply(&mut caches.stamp_state,result,&mut ui_mgr,caches.time.elapsed_secs_f64());
        let property_forwarded = forward_correlated_property_reply(result, &mut pending_api);
        let materialization = match result {
            DbResult::NodeMaterialized { node, .. } => Some(node.clone()),
            DbResult::GltfImported { .. } => {
                let current=committed::from_gltf_import(result).cloned();
                if current.is_none() {
                    bevy::log::warn!("Asset placement receipt needs authoritative hierarchy hydration");
                    request_committed_hierarchy(&mut caches.committed_hierarchy_pending, &mut nav);
                }
                current
            }
            DbResult::NodeMutationCommitted { receipt, .. } => {
                let current = committed::from_receipt(receipt, &verse_mgr);
                if current.is_none() {
                    bevy::log::warn!(
                        "Current object receipt needs authoritative hierarchy hydration"
                    );
                    request_committed_hierarchy(&mut caches.committed_hierarchy_pending, &mut nav);
                }
                current
            }
            _ => None,
        };
        if let Some(current) = materialization {
            let url_model_changed = verse_mgr.url_asset_ids.get(&current.node.node_id) != Some(&current.asset_id);
            let previous_url = verse_mgr.all_nodes().find(|node| node.id == current.node.node_id).and_then(|node| node.webpage_url.clone());
            let outcome = committed::apply(
                &current,
                &mut verse_mgr,
                &mut commands,
                &caches.current_nodes,
            );
            let transform_changed = match outcome {
                committed::MaterializationOutcome::Applied { transform_changed } => {
                    Some(transform_changed)
                }
                committed::MaterializationOutcome::NeedsHierarchy => {
                    request_committed_hierarchy(&mut caches.committed_hierarchy_pending, &mut nav);
                    None
                }
                committed::MaterializationOutcome::Ignored => None,
            };
            if let Some(transform_changed) = transform_changed {
                if url_model_changed && (current.node.has_asset || previous_url.is_some()) && !ui_mgr.url_read_model.is_reading(&current.node.node_id) {
                    if let Err(reason) = crate::actions::url_read_model::request_after_model_change(&db_sender.0,
                        &mut ui_mgr, &verse_mgr, &nav, &node_mgr, &inspector, &current.node.node_id,
                        current.asset_id.clone(), previous_url.as_deref(), caches.time.elapsed_secs_f64()) {
                        ui_mgr.show_advisory(reason, caches.time.elapsed_secs_f64());
                    }
                }
                if transform_changed && node_mgr.accept_committed_transform(&current.node.node_id) {
                    ui_mgr.show_advisory(
                        "Object updated; unfinished move cancelled",
                        caches.time.elapsed_secs_f64(),
                    );
                }
                properties::handle_node_properties_loaded(
                    &current.node.node_id,
                    Some(&current.node.petal_id),
                    &current.properties,
                    &mut path_state,
                    &node_mgr,
                    &mut inspector,
                    &mut caches.primitive,
                    &mut caches.path_asset,
                    &mut caches.stamp_state,
                    &mut caches.earthwork_map,
                    &mut caches.visibility,
                    &mut caches.callout_map,
                );
            }
        }
        inspector
            .query_results
            .invalidate(nav.active_verse_id.as_deref());
        if tokens::handle_inspector_token_failure(
            result,
            node_mgr.selected.as_ref().map(|selection| selection.node_id.as_str()),
            &crate::panels::inspector::build_nav_scope(&nav),
            local_role.can_manage(),
            &mut inspector,
        ) { continue; }
        match result {
            DbResult::Seeded { .. } => hierarchy::handle_seeded(&db_sender),
            DbResult::HierarchyLoaded { verses } => {
                // Phase 4: the persisted verse/fractal/petal eyes ride the tree.
                crate::visibility::groups::hydrate_scope_hidden(&mut caches.visibility, verses);
                hierarchy::handle_hierarchy_loaded(
                    verses,
                    &mut verse_mgr,
                    &mut nav,
                    &mut commands,
                    &asset_server,
                    &mut pending_api,
                    &mut already_spawned,
                    mesh_budget_exceeded,
                    &caches.residency,
                    &mut caches.ts_settings,
                )
            }
            DbResult::VerseJoined {
                verse_id,
                verse_name,
            } => hierarchy::handle_verse_joined(
                verse_id,
                verse_name,
                &db_sender,
                &mut nav,
                &mut ui_mgr,
                caches.time.elapsed_secs_f64(),
            ),
            // OD-2/M-03: typed Join failure — inline in the dialog while
            // pending, else a general error notice (see hierarchy.rs).
            DbResult::JoinVerseFailed { error } => hierarchy::handle_join_verse_failed(
                error,
                &mut ui_mgr,
                caches.time.elapsed_secs_f64(),
            ),
            // Still falls through to `try_deliver`: the API caller awaits it.
            DbResult::ElevatedQueryResult { changed_rows, .. } => {
                hierarchy::handle_elevated_query_result(*changed_rows, &db_sender)
            }
            DbResult::DatabaseReset { .. } => {
                caches.primitive.clear();
                caches.path_asset.clear();
                hierarchy::handle_database_reset(&mut verse_mgr, &db_sender);
            }
            DbResult::VerseCreated {
                id,
                name,
                namespace_id,
            } => hierarchy::handle_verse_created(id, name, namespace_id.clone(), &mut verse_mgr),
            DbResult::FractalCreated { id, verse_id, name } => {
                hierarchy::handle_fractal_created(id, verse_id, name, &mut verse_mgr)
            }
            DbResult::PetalCreated {
                id,
                fractal_id,
                name,
            } => hierarchy::handle_petal_created(id, fractal_id, name, &mut verse_mgr),
            DbResult::PetalRequirementsLoaded {snapshot,correlation_id} => {
                crate::dialogs::requirements_state::apply(&mut ui_mgr,&nav,correlation_id,Ok(snapshot));
            }
            DbResult::EntityDetailsLoaded {
                entity_type,
                entity_id,
                request_id,
                details,
            } => {
                crate::dialogs::entity_details::apply(
                    &mut ui_mgr,
                    *entity_type,
                    entity_id,
                    *request_id,
                    details,
                    caches.time.elapsed_secs_f64(),
                );
            }
            DbResult::EntityRenamed {
                entity_type,
                entity_id,
                new_name,
            } => {
                hierarchy::handle_entity_renamed(
                    entity_type,
                    entity_id,
                    new_name,
                    &mut verse_mgr,
                    &mut nav,
                );
                crate::dialogs::entity_details::refresh_target(
                    &mut ui_mgr,
                    *entity_type,
                    entity_id,
                );
            }
            DbResult::EntityDeleted {
                entity_type,
                entity_id,
            } => hierarchy::handle_entity_deleted(
                entity_type,
                entity_id,
                &mut verse_mgr,
                &mut nav,
                &mut ui_mgr,
            ),
            DbResult::GltfImported {
                request: Some(request), name, correlation_id: None, petal_id, ..
            } => {
                if request.petal_id == *petal_id && committed::from_gltf_import(result).is_some() {
                    finish_gltf_import_request(request, name, &mut ui_mgr, &nav, caches.time.elapsed_secs_f64());
                }
            }
            DbResult::ImportGltfFailed { request, message } => query::handle_import_failure(
                request.as_ref(), message, &mut ui_mgr, &local_role, &nav, caches.time.elapsed_secs_f64(),
            ),
            DbResult::NodeCreated {
                id,
                petal_id,
                name,
                has_asset,
                correlation_id,
                position,
            } => nodes::handle_node_created(
                id,
                petal_id,
                name,
                *has_asset,
                correlation_id.as_deref(),
                *position,
                &mut verse_mgr,
                &nav,
                &mut ui_mgr,
                &mut path_state,
                &db_sender,
                &mut caches.earthwork_map,
                &mut caches.callout_map,
            ),
            DbResult::NodeDeleted { node_id, petal_id }
            | DbResult::PluginNodeRemoved { node_id, petal_id } => nodes::handle_node_deleted(
                node_id,
                petal_id,
                &mut verse_mgr,
                &nav,
                &mut path_state,
                &db_sender,
                &mut caches.path_asset,
                &mut caches.applied,
                &mut caches.callout_map,
            ),
            // T4: node display-name update (mirrors EntityRenamed for the tree).
            DbResult::NodeRenamed { node_id, new_name } => {
                nodes::handle_node_renamed(node_id, new_name, &mut verse_mgr)
            }
            // T4: authoritative subtree size for the cascade-delete confirm.
            DbResult::NodeDescendantCount { node_id, count } => {
                nodes::handle_node_descendant_count(node_id, *count, &mut ui_mgr)
            }
            // T2 FR-5: a completed PromoteInstance — record the stamp's node id
            // and flush overrides buffered before it was known (§stamped-assets).
            DbResult::NodePromoted {
                node_id,
                path_id,
                instance_index,
                ..
            } => crate::actions::asset::handle_node_promoted(
                &db_sender,
                &mut caches.stamp_state,
                node_id,
                path_id,
                *instance_index as usize,
            ),
            DbResult::VerseInviteGenerated { invite_string, .. } => {
                roles::handle_verse_invite_generated(invite_string, &mut ui_mgr,caches.time.elapsed_secs_f64())
            }
            DbResult::EntityAccessFinished {target,request_id,result} => {
                crate::dialogs::entity_access::apply(&mut ui_mgr,target,request_id,result);
            }
            DbResult::PeerRolesResolved { scope, roles } => {
                roles::handle_peer_roles_resolved(scope, roles, &mut ui_mgr)
            }
            DbResult::RoleAssigned {
                peer_did,
                scope,
                role,
            } => roles::handle_role_assigned(peer_did, scope, role, &mut ui_mgr),
            DbResult::RoleRevoked { peer_did, scope } => {
                roles::handle_role_revoked(peer_did, scope, &mut ui_mgr)
            }
            DbResult::ScopedInviteGenerated { invite_link } => {
                roles::handle_scoped_invite_generated(invite_link, &mut ui_mgr,caches.time.elapsed_secs_f64())
            }
            DbResult::LocalRoleResolved { scope, role } => {
                roles::handle_local_role_resolved(scope, role, &nav, &mut local_role)
            }
            DbResult::VerseDefaultAccessSet {
                verse_id,
                default_access,
            } => {
                roles::handle_verse_default_access_set(verse_id, default_access);
                crate::dialogs::entity_details::refresh_target(
                    &mut ui_mgr,
                    fe_runtime::messages::EntityType::Verse,
                    verse_id,
                );
            }
            DbResult::OrphanedVerseClaimed {
                verse_id,
                old_did,
                new_did,
                ..
            } => roles::handle_orphaned_verse_claimed(
                verse_id,
                old_did,
                new_did,
                &nav,
                &mut local_role,
                &mut ui_mgr,
                caches.time.elapsed_secs_f64(),
            ),
            DbResult::OrphanedVerseClaimRefused {
                verse_id, reason, ..
            } => roles::handle_orphaned_verse_claim_refused(
                verse_id,
                reason,
                &mut ui_mgr,
                caches.time.elapsed_secs_f64(),
            ),
            // M2/F6: authoritative persisted timeseries settings — fold into the
            // settings surface's per-verse map (A13).
            DbResult::VerseTimeseriesSettingsSet {
                verse_id,
                mode,
                replication_factor,
                bucket_width_ms,
            } => roles::handle_verse_timeseries_settings_set(
                verse_id,
                mode,
                *replication_factor,
                *bucket_width_ms,
                &mut caches.ts_settings,
            ),
            DbResult::FractalDescriptionUpdated {
                fractal_id,
                description,
            } => {
                roles::handle_fractal_description_updated(fractal_id, description);
                crate::dialogs::entity_details::refresh_target(
                    &mut ui_mgr,
                    fe_runtime::messages::EntityType::Fractal,
                    fractal_id,
                );
            }
            DbResult::ApiTokenMinted { .. } => {
                if tokens::handle_api_token_minted(
                    result,
                    node_mgr.selected.as_ref().map(|selection| selection.node_id.as_str()),
                    &crate::panels::inspector::build_nav_scope(&nav),
                    local_role.can_manage(),
                    &mut ui_mgr,
                    &mut inspector,
                    &db_sender,
                ) { continue; }
            }
            DbResult::ApiTokenRevoked { jti } => tokens::handle_api_token_revoked(
                jti,
                revocation_tx.as_deref(),
                &inspector,
                &db_sender,
            ),
            DbResult::ApiTokensListed { tokens, total } => {
                tokens::handle_api_tokens_listed(tokens, *total, &mut ui_mgr, &mut inspector)
            }
            DbResult::ScopedApiTokensListed { tokens, total } => {
                tokens::handle_scoped_api_tokens_listed(tokens, *total, &mut ui_mgr, &mut inspector)
            }
            // GUI queries are correlated (`ui-*` ids): a reply lands only in
            // the buffer holding its id; anything unclaimed (an API waiter's
            // ULID) falls through to `try_deliver` (M4 fix B2).
            DbResult::QueryResult {
                data,
                correlation_id: Some(id),
            } => {
                if query::route_query_reply(
                    id,
                    Ok(data),
                    &mut gis_panel,
                    &mut path_state,
                    &mut inspector,
                ) {
                    continue;
                }
            }
            DbResult::QueryFailed {
                error,
                correlation_id,
            } => {
                if query::route_query_reply(
                    correlation_id,
                    Err(error),
                    &mut gis_panel,
                    &mut path_state,
                    &mut inspector,
                ) {
                    continue;
                }
            }
            DbResult::QueryResult {
                data,
                correlation_id: None,
            } => query::handle_query_result(data, &mut gis_panel, &mut path_state),
            DbResult::Error(msg) => query::handle_error(
                msg,
                &mut path_state,
                &mut ui_mgr,
                &local_role,
                &nav,
                caches.time.elapsed_secs_f64(),
            ),
            // API delivery precedes selection guards; cache hydration still runs.
            DbResult::NodePropertiesLoaded {
                node_id,
                properties,
                ..
            } => {
                // Resolve the node's owning petal so the path-asset cache entry
                // knows where to materialize (borrow ends before the handler call).
                let petal_id = verse_mgr.petal_id_of(node_id).map(str::to_string);
                if !properties::handle_node_properties_loaded(
                    node_id,
                    petal_id.as_deref(),
                    properties,
                    &mut path_state,
                    &node_mgr,
                    &mut inspector,
                    &mut caches.primitive,
                    &mut caches.path_asset,
                    &mut caches.stamp_state,
                    &mut caches.earthwork_map,
                    &mut caches.visibility,
                    &mut caches.callout_map,
                ) {
                    continue;
                }
            }
            DbResult::NodeViewSet {
                node_id,
                state,
                groups,
                ..
            } => crate::visibility::cache::apply_node_view_echo(
                &mut caches.visibility,
                node_id,
                state.as_deref(),
                groups.as_deref(),
            ),
            DbResult::PetalViewStateLoaded { entries, .. } => {
                crate::visibility::cache::apply_petal_view_state(&mut caches.visibility, entries)
            }
            DbResult::PetalVisibilityGroupsLoaded {
                petal_id, groups, ..
            } => crate::visibility::groups::apply_registry_loaded(
                &mut caches.visibility,
                nav.active_petal_id.as_deref(),
                petal_id,
                groups,
            ),
            DbResult::ScopeHiddenSet {
                level, id, hidden, ..
            } => caches.visibility.set_scope_hidden(*level, id, *hidden),
            DbResult::RequestFailed {
                error,
                correlation_id,
            } => {
                crate::dialogs::requirements_state::apply(&mut ui_mgr,&nav,correlation_id,Err(error));
                if let Some(toast) =
                    crate::visibility::cache::view_write_failure_toast(correlation_id, error)
                        .or_else(|| {
                            crate::visibility::groups::registry_write_failure_toast(
                                correlation_id,
                                error,
                            )
                        })
                {
                    bevy::log::warn!("visibility write not persisted: {error}");
                    ui_mgr.show_error(toast, caches.time.elapsed_secs_f64());
                }
            }
            DbResult::NodePropertySet { node_id, key, .. } => {
                if !properties::handle_node_property_set(
                    node_id,
                    key,
                    &nav,
                    &db_sender,
                    &mut path_state,
                    &node_mgr,
                    &mut inspector,
                    &mut caches.primitive,
                ) {
                    continue;
                }
            }
            DbResult::NodePropertyDeleted { node_id, key, .. } => {
                if !properties::handle_node_property_deleted(
                    node_id,
                    key,
                    &node_mgr,
                    &mut inspector,
                    &mut caches.primitive,
                    &mut caches.path_asset,
                ) {
                    continue;
                }
            }
            DbResult::FieldDefsListed { scope, field_defs } => {
                if nav.active_scope().as_deref() == Some(scope.as_str()) {
                    fields::handle_field_defs_listed(field_defs, &mut inspector);
                }
            }
            // Field-def mutations only need a re-list, which the panel re-sends itself.
            DbResult::FieldDefCreated { .. }
            | DbResult::FieldDefUpdated { .. }
            | DbResult::FieldDefDeleted { .. } => {
                if let Some(scope) = nav.active_scope() {
                    if db_sender
                        .0
                        .send(fe_runtime::messages::DbCommand::ListFieldDefs { scope })
                        .is_err()
                    {
                        bevy::log::warn!(
                            "DB channel closed while refreshing committed field definitions"
                        );
                    } else {
                        inspector.field_defs_loading = true;
                    }
                }
            }
            // Both GUI and API-initiated (correlated) sets apply here, so the
            // host's live terrain tracks every write (fe-runtime §api-reply-correlation).
            // M-03: the correlation id is threaded through so a GUI-correlated
            // echo (minted by `set_petal_map`) can clear `PetalMapState::pending_set`
            // (W1-S3's rollback contract) — a foreign/API correlation is W1-S3's own
            // concern inside the handler, not filtered here.
            DbResult::PetalTerrainLoaded {
                petal_id,
                terrain,
                correlation_id,
            } => terrain::handle_petal_terrain_loaded(
                petal_id,
                terrain,
                &nav,
                &mut petal_map,
                &mut caches.proposal_state,
                correlation_id.as_deref(),
            ),
            // M-03: a correlated (GUI-minted) `SetPetalTerrain` failure — roll
            // back the optimistic Maps-card update and explain the denial.
            DbResult::PetalTerrainSetFailed {
                petal_id,
                error,
                correlation_id,
            } => terrain::handle_petal_terrain_set_failed(
                petal_id,
                error,
                correlation_id,
                &mut petal_map,
                &mut ui_mgr,
                &local_role,
                crate::notice::active_verse_name(&nav),
                caches.time.elapsed_secs_f64(),
            ),
            // p2p_asset_streaming S6: both the GUI's own echo and a
            // ReplicatedRowApplied-triggered re-fetch (below) land here.
            DbResult::PetalRenderDistanceLoaded {
                petal_id,
                render_distance,
                ..
            } => terrain::handle_petal_render_distance_loaded(
                petal_id,
                *render_distance,
                &mut verse_mgr,
            ),
            // S6: a peer's converged `petal` row — re-fetch render_distance
            // so a peer's change lands without a full hierarchy reload
            // (fe-ui/src/AGENTS.md §app-settings). Not the terrain field: a
            // peer's converged `terrain` reaches this GUI on the next petal
            // entry (`GetPetalTerrain`), not live — re-fetching it here would
            // rebuild the whole terrain on every peer petal-row write.
            DbResult::ReplicatedRowApplied {
                table,
                record_id,
                outcome: fe_runtime::messages::ReplicatedRowOutcome::Applied,
                ..
            } if table.as_str() == "petal" => {
                if let Err(e) =
                    db_sender
                        .0
                        .send(fe_runtime::messages::DbCommand::GetPetalRenderDistance {
                            petal_id: record_id.clone(),
                            correlation_id: None,
                        })
                {
                    bevy::log::warn!(
                        "db_sender channel closed — GetPetalRenderDistance re-fetch dropped for {record_id}: {e}"
                    );
                }
            }
            _ => {}
        }

        // Also try delivering every result to pending API requests.
        // This covers cases like ScopeResolved, NodeCreated, etc. that
        // the API thread may be waiting on.
        if !property_forwarded {
            pending_api.try_deliver(result.clone());
        }
    }
    flush_committed_hierarchy(&db_sender, &mut caches.committed_hierarchy_pending);
}

/// Finish only the current native import; stale committed results still hydrate above.
fn finish_gltf_import_request(
    request: &fe_runtime::messages::NativeImportRequest,
    name: &str,
    ui_mgr: &mut crate::actions::UiManager,
    nav: &NavigationManager,
    now: f64,
) {
    if matches!(
        ui_mgr.active_dialog,
        crate::dialogs::ActiveDialog::GltfImport { .. }
    ) && ui_mgr.gltf_import_request.finish(request, nav)
    {
        ui_mgr.close_dialog();
        ui_mgr.show_toast(format!("Imported {name}"), now);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
    use super::*;
    use crate::actions::{UiAction, UiManager};
    use crate::dialogs::ActiveDialog;
    use crate::gis::{CornerKind, GisPanelState, PathEditorState, PendingPenCreate};
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
                        render_distance: None,
                        nodes: vec![NodeEntry {
                            transform: Default::default(),
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
        assert!(properties::is_for_selected_node(
            &mgr_selected("node-1"),
            "node-1"
        ));
    }

    #[test]
    fn is_for_selected_node_false_when_different_node() {
        assert!(!properties::is_for_selected_node(
            &mgr_selected("node-1"),
            "node-2"
        ));
    }

    #[test]
    fn is_for_selected_node_false_when_nothing_selected() {
        assert!(!properties::is_for_selected_node(
            &NodeManager::default(),
            "node-1"
        ));
    }

    // --- hierarchy handlers ---

    #[test]
    fn seeded_and_verse_joined_send_load_hierarchy() {
        let (tx, rx) = sender();
        hierarchy::handle_seeded(&tx);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
        let mut nav = NavigationManager::default();
        let mut ui = UiManager::default();
        hierarchy::handle_verse_joined("v1", "V1", &tx, &mut nav, &mut ui, 0.0);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
    }

    /// OD-2/M-03: a pending Join dialog closes, navigates and toasts on a
    /// successful `VerseJoined` — the hierarchy reload still always fires.
    #[test]
    fn join_success_navigates_and_toasts() {
        let (tx, rx) = sender();
        let mut nav = NavigationManager::default();
        let mut ui = UiManager::default();
        ui.join_request.pending = true;
        ui.open_dialog(ActiveDialog::JoinDialog {
            invite_buf: "fractalengine://invite/abc".into(),
        });
        hierarchy::handle_verse_joined("v2", "Joined Verse", &tx, &mut nav, &mut ui, 10.0);
        assert_eq!(nav.active_verse_id.as_deref(), Some("v2"));
        assert_eq!(nav.active_verse_name, "Joined Verse");
        assert!(
            matches!(ui.active_dialog, ActiveDialog::None),
            "the Join dialog closes on success"
        );
        assert!(!ui.join_request.pending);
        assert!(matches!(rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
        assert!(
            ui.active_toast(10.0)
                .is_some_and(|(msg, _)| msg.contains("Joined Verse")),
            "a success toast names the joined verse"
        );
    }

    /// OD-2/M-03: a `JoinVerseFailed` while the dialog is pending reports
    /// inline (the dialog stays open) rather than vanishing into a toast.
    #[test]
    fn join_failure_keeps_dialog_open_with_inline_error() {
        let mut ui = UiManager::default();
        ui.join_request.pending = true;
        ui.open_dialog(ActiveDialog::JoinDialog {
            invite_buf: "garbage".into(),
        });
        hierarchy::handle_join_verse_failed(
            "invalid invite link: must start with 'fractalengine://invite/'",
            &mut ui,
            5.0,
        );
        assert!(
            !ui.join_request.pending,
            "pending clears so the next Join click starts fresh"
        );
        assert!(
            ui.join_request.error.is_some(),
            "the humanized error lands inline for the dialog to render"
        );
        assert!(
            matches!(ui.active_dialog, ActiveDialog::JoinDialog { .. }),
            "the dialog stays open so the user sees the inline error"
        );
    }

    /// D-06: the Import dialog's own `GltfImported` closes it and confirms;
    /// an import nobody in this dialog asked for leaves the open dialog be.
    #[test]
    fn gltf_import_reply_closes_only_the_dialog_that_asked() {
        let dialog = || ActiveDialog::GltfImport {
            file_path_buf: "model.glb".into(),
            name_buf: "Model".into(),
            position: [0.0; 3],
        };
        let request = fe_runtime::messages::NativeImportRequest {
            request_id: "old".into(),
            verse_id: "v1".into(),
            fractal_id: "f1".into(),
            petal_id: "p1".into(),
        };
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal_in_fractal("v1", "Verse", "f1", "Fractal", "p1");
        let mut ui = UiManager::default();
        ui.open_dialog(dialog());
        ui.gltf_import_request.start(request.clone(), 1.0);
        ui.gltf_import_request.expire(32.0, 30.0);
        assert!(ui.gltf_import_request.pending && ui.gltf_import_request.outcome_unknown);
        assert!(
            ui.gltf_import_request
                .error
                .as_deref()
                .unwrap()
                .contains("unknown")
        );
        finish_gltf_import_request(&request, "Model", &mut ui, &nav, 33.0);
        assert!(matches!(ui.active_dialog, ActiveDialog::None));
        assert_eq!(
            ui.active_toast(33.0).map(|(msg, _)| msg.to_string()),
            Some("Imported Model".into())
        );
        ui.open_dialog(dialog());
        ui.gltf_import_request.start(request.clone(), 34.0);
        ui.open_dialog(dialog());
        let current = fe_runtime::messages::NativeImportRequest {
            request_id: "new".into(),
            ..request.clone()
        };
        ui.gltf_import_request.start(current.clone(), 35.0);
        finish_gltf_import_request(&request, "Old", &mut ui, &nav, 36.0);
        assert!(
            ui.gltf_import_request.pending
                && matches!(ui.active_dialog, ActiveDialog::GltfImport { .. })
        );
        nav.navigate_to_petal("p2");
        finish_gltf_import_request(&current, "Wrong target", &mut ui, &nav, 36.0);
        assert!(ui.gltf_import_request.pending);
        nav.navigate_to_petal("p1");
        finish_gltf_import_request(&current, "Current", &mut ui, &nav, 37.0);
        assert!(matches!(ui.active_dialog, ActiveDialog::None));
        ui.open_dialog(dialog());
        ui.gltf_import_request.start(request.clone(), 38.0);
        ui.open_dialog(ActiveDialog::JoinDialog {
            invite_buf: String::new(),
        });
        finish_gltf_import_request(&request, "Old", &mut ui, &nav, 39.0);
        assert!(
            !ui.gltf_import_request.pending
                && matches!(ui.active_dialog, ActiveDialog::JoinDialog { .. })
        );
    }

    /// A foreign `JoinVerseFailed` (no dialog open, or already closed) must
    /// not be dropped silently — it surfaces as a general error notice.
    #[test]
    fn join_failure_without_pending_dialog_shows_error_notice() {
        let mut ui = UiManager::default();
        hierarchy::handle_join_verse_failed("some DB-thread failure", &mut ui, 3.0);
        assert!(ui.join_request.error.is_none());
        assert!(
            ui.active_notice(3.0).is_some(),
            "a stale/foreign join failure still reaches the user"
        );
    }

    #[test]
    fn only_an_elevated_write_that_changed_rows_reloads_the_tree() {
        let (tx, rx) = sender();
        hierarchy::handle_elevated_query_result(0, &tx);
        assert!(rx.try_recv().is_err(), "a read-only script changes nothing");
        hierarchy::handle_elevated_query_result(3, &tx);
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
        hierarchy::handle_verse_created("v2", "V2", Some("ns-v2".to_string()), &mut mgr);
        assert_eq!(mgr.verses.len(), 2);
        assert_eq!(
            mgr.verses[1].namespace_id.as_deref(),
            Some("ns-v2"),
            "VerseCreated carries the namespace_id onto the tree entry"
        );
        hierarchy::handle_fractal_created("f2", "v2", "F2", &mut mgr);
        assert_eq!(mgr.verses[1].fractals.len(), 1);
        hierarchy::handle_petal_created("p2", "f2", "P2", &mut mgr);
        assert_eq!(mgr.verses[1].fractals[0].petals.len(), 1);
    }

    /// F22 end-to-end GUI path: a verse created through the real create flow
    /// has NO namespace_id until `DbResult::VerseCreated` arrives; the result
    /// carries the DB-computed id, `handle_verse_created` puts it on the tree
    /// entry, and navigating into the verse then sends the `OpenVerseReplica`
    /// that unblocks the F21-retained manifest write on the sync seam. This
    /// pins the REAL fresh-create state (the old test fabricated
    /// `namespace_id: Some(...)` directly in the tree).
    #[test]
    fn fresh_created_verse_then_navigation_opens_replica() {
        use bevy::prelude::*;
        use fe_identity::{InMemoryBackend, SecretStore};
        use fe_sync::SyncCommand;
        use std::sync::Arc;

        let verse_id = "01F22GUIOPENVARSE0000000000";
        let secret = [5u8; 32];
        let hex_secret = fe_database::hash_to_hex(&secret);
        let ns_id = fe_database::hash_to_hex(&fe_database::derive_namespace_id(&secret));

        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        store
            .set(
                &format!("fractalengine:verse:{verse_id}:ns_secret"),
                "fractalengine",
                &hex_secret,
            )
            .unwrap();

        // Real create flow: the tree has no verse until VerseCreated arrives
        // (the create dialog only sends CreateVerse; nothing is pushed
        // locally).
        let mut mgr = VerseManager::default();
        assert!(
            mgr.verses.is_empty(),
            "no verse in the tree before the result lands"
        );

        // `DbResult::VerseCreated` arrives carrying the DB-computed namespace
        // id — exactly the dispatcher arm's call.
        hierarchy::handle_verse_created(verse_id, "GUI Fresh Verse", Some(ns_id.clone()), &mut mgr);
        assert_eq!(
            mgr.verses
                .iter()
                .find(|v| v.id == verse_id)
                .and_then(|v| v.namespace_id.clone()),
            Some(ns_id.clone()),
            "VerseCreated populates the tree entry's namespace_id"
        );

        let (sync_tx, sync_rx) = crossbeam::channel::bounded(4);
        let mut app = App::new();
        app.init_resource::<NavigationManager>();
        app.insert_resource(mgr);
        app.insert_resource(fe_sync::SyncCommandSenderRes(sync_tx));
        app.insert_resource(fe_database::SecretStoreRes(store));
        app.add_systems(
            Update,
            crate::navigation_manager::handle_verse_replica_lifecycle,
        );

        // Frame 1: lifecycle initializes with nothing active.
        app.update();
        assert!(sync_rx.try_recv().is_err(), "no open before navigation");

        // Frame 2: the user navigates into the freshly created verse — the
        // open that flushes the retained manifest through the F21 seam.
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse(verse_id, "GUI Fresh Verse");
        app.update();

        match sync_rx
            .try_recv()
            .expect("OpenVerseReplica after navigating into the fresh verse")
        {
            SyncCommand::OpenVerseReplica {
                verse_id: v,
                namespace_id,
                namespace_secret,
                bootstrap_peers,
            } => {
                assert_eq!(v, verse_id);
                assert_eq!(namespace_id, ns_id);
                assert_eq!(namespace_secret, Some(hex_secret));
                assert!(bootstrap_peers.is_empty());
            }
            other => panic!("unexpected command: {other:?}"),
        }

        use fe_runtime::messages::{
            FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
        };
        let empty = VerseHierarchyData {
            id: "empty-verse".into(),
            name: "Empty".into(),
            namespace_id: None,
            timeseries: Default::default(),
            view_hidden: false,
            fractals: vec![],
        };
        let mut populated = empty.clone();
        populated.id = "populated-verse".into();
        populated.name = "Populated".into();
        populated.fractals = vec![FractalHierarchyData {
            id: "f".into(),
            name: "Fractal".into(),
            view_hidden: false,
            petals: vec![PetalHierarchyData {
                id: "p".into(),
                name: "Petal".into(),
                view_hidden: false,
                render_distance: None,
                nodes: vec![NodeHierarchyData {
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    id: "n".into(),
                    name: "Node".into(),
                    has_asset: true,
                    position: [0.0; 3],
                    asset_path: Some("fixture.glb".into()),
                    petal_id: "p".into(),
                    webpage_url: None,
                }],
            }],
        }];
        let mut nav = NavigationManager::default();
        hierarchy::select_initial_navigation(&[empty.clone(), populated], &mut nav);
        assert_eq!(nav.active_verse_name, "Populated");
        assert_eq!(
            nav.active_scope().as_deref(),
            Some("VERSE#populated-verse-FRACTAL#f-PETAL#p")
        );
        hierarchy::select_initial_navigation(&[empty], &mut nav);
        assert_eq!(nav.active_scope().as_deref(), Some("VERSE#empty-verse"));
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
        assert!(
            mgr.node_index.is_empty(),
            "index must be rebuilt after structural delete"
        );
        assert!(matches!(ui.active_dialog, ActiveDialog::None));

        let mut mgr = tree();
        nav.navigate_to_petal_in_fractal("v1", "Verse", "f1", "Fractal", "p1");
        hierarchy::handle_entity_deleted(&EntityType::Fractal, "f1", &mut mgr, &mut nav, &mut ui);
        assert!(mgr.verses[0].fractals.is_empty());
        assert!(nav.active_petal_id.is_none());
        assert_eq!(nav.active_scope().as_deref(), Some("VERSE#v1"));
    }

    // --- node handlers ---

    #[test]
    fn node_created_adds_node_and_indexes_it() {
        let (tx, rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default(); // no active petal → no Paths re-query
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        nodes::handle_node_created(
            "n2",
            "p1",
            "New",
            false,
            None,
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(mgr.all_nodes().any(|n| n.id == "n2"));
        assert!(mgr.node_index.contains_key("n2"));
        assert!(
            rx.try_recv().is_err(),
            "inactive petal must not re-query tracks"
        );
    }

    #[test]
    fn node_created_in_active_petal_reruns_paths_query() {
        let (tx, rx) = sender();
        let mut mgr = tree();
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        nodes::handle_node_created(
            "n2",
            "p1",
            "New",
            false,
            None,
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(
            rx.try_recv().is_ok(),
            "active-petal create must re-run the Paths query"
        );
    }

    #[test]
    fn node_created_with_earthwork_correlation_binds_map_and_writes_contract() {
        // T3 (D-A8/N-10): an `earthwork:{region_id}` echo binds region→node and
        // writes the endpoint property bag (stashed material consumed once).
        let (tx, rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default(); // inactive petal — no Paths re-query noise
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        let mut map = crate::actions::terrain_proposal::EarthworkNodeMap::default();
        map.stash_pending_material("r2", "gravel");
        nodes::handle_node_created(
            "node-7",
            "p1",
            "Earthwork raise r2",
            false,
            Some("earthwork:r2"),
            [3.0, 0.0, 4.0],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut map,
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert_eq!(map.node_for("r2"), Some("node-7"));
        let mut keys = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            if let DbCommand::SetNodeProperty {
                node_id,
                key,
                value,
                correlation_id: None,
            } = cmd
            {
                assert_eq!(node_id, "node-7");
                if key == "material" {
                    assert_eq!(value, json!("gravel"), "stashed material consumed");
                }
                if key == "node_kind" {
                    assert_eq!(value, json!("earthwork_region"));
                }
                keys.push(key);
            }
        }
        for expected in [
            "node_kind",
            "material",
            "region_id",
            "cut_volume_m3",
            "fill_volume_m3",
        ] {
            assert!(keys.iter().any(|k| k == expected), "missing key {expected}");
        }
    }

    // --- pen auto-create flush (pen_autocreate_track FR-2 + pen_curve FR-4) ---

    /// Handle-less pending stash; tests layer bezier fields via struct update.
    fn pen_stash(cid: &str) -> PendingPenCreate {
        PendingPenCreate {
            correlation_id: cid.into(),
            first_point: [1.0, 2.0, 3.0],
            ..Default::default()
        }
    }

    #[test]
    fn pen_flush_with_handles_appends_smooth_point_with_all_fields() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.pending_pen_create = Some(PendingPenCreate {
            handle_in: Some([-4.0, 0.5, -5.0]),
            handle_out: Some([4.0, -0.5, 5.0]),
            corner: CornerKind::Symmetric,
            smoothness: 0.42,
            ..pen_stash("pen-track:t1")
        });
        nodes::handle_node_created(
            "n2",
            "p1",
            "Track",
            false,
            Some("pen-track:t1"),
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert_eq!(path_state.editing_track_id.as_deref(), Some("n2"));
        assert!(
            !path_state.has_pending_pen_create(),
            "flush must consume the stash"
        );
        let actions = ui.drain_actions();
        assert_eq!(actions.len(), 1, "exactly one deferred first-point append");
        match &actions[0] {
            UiAction::PathAppendSmoothPoint {
                track_node_id,
                position,
                handle_in,
                handle_out,
                corner,
                smoothness,
            } => {
                assert_eq!(track_node_id, "n2", "flushes onto the echoed node id");
                assert_eq!(*position, [1.0, 2.0, 3.0], "raw petal-local meters");
                assert_eq!(*handle_in, Some([-4.0, 0.5, -5.0]));
                assert_eq!(*handle_out, Some([4.0, -0.5, 5.0]));
                assert_eq!(*corner, CornerKind::Symmetric);
                assert_eq!(*smoothness, 0.42);
            }
            other => panic!("expected PathAppendSmoothPoint, got {other:?}"),
        }
    }

    #[test]
    fn pen_flush_with_one_sided_handle_still_takes_smooth_branch() {
        // A press-drag first anchor typically carries only handle_out.
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.pending_pen_create = Some(PendingPenCreate {
            handle_out: Some([2.0, 0.0, 2.0]),
            corner: CornerKind::Smooth,
            smoothness: 1.0,
            ..pen_stash("pen-track:t2")
        });
        nodes::handle_node_created(
            "n2",
            "p1",
            "Track",
            false,
            Some("pen-track:t2"),
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        let actions = ui.drain_actions();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            UiAction::PathAppendSmoothPoint {
                handle_in,
                handle_out,
                corner,
                smoothness,
                ..
            } => {
                assert_eq!(*handle_in, None, "absent side must stay absent");
                assert_eq!(*handle_out, Some([2.0, 0.0, 2.0]));
                assert_eq!(*corner, CornerKind::Smooth);
                assert_eq!(*smoothness, 1.0);
            }
            other => panic!("expected PathAppendSmoothPoint, got {other:?}"),
        }
    }

    #[test]
    fn pen_flush_without_handles_appends_plain_point() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.pending_pen_create = Some(pen_stash("pen-track:t3"));
        nodes::handle_node_created(
            "n2",
            "p1",
            "Track",
            false,
            Some("pen-track:t3"),
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert_eq!(path_state.editing_track_id.as_deref(), Some("n2"));
        assert!(!path_state.has_pending_pen_create());
        let actions = ui.drain_actions();
        assert_eq!(actions.len(), 1, "plain click stays a single legacy append");
        match &actions[0] {
            UiAction::PathAppendPoint {
                track_node_id,
                position,
            } => {
                assert_eq!(track_node_id, "n2");
                assert_eq!(*position, [1.0, 2.0, 3.0]);
            }
            other => panic!("expected legacy PathAppendPoint, got {other:?}"),
        }
    }

    #[test]
    fn pen_flush_skipped_on_correlation_mismatch() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.pending_pen_create = Some(PendingPenCreate {
            handle_out: Some([1.0, 0.0, 0.0]),
            ..pen_stash("pen-track:t4")
        });
        nodes::handle_node_created(
            "n2",
            "p1",
            "Foreign",
            false,
            Some("authored-track:9"),
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(
            ui.drain_actions().is_empty(),
            "foreign echo must not flush the pen point"
        );
        assert!(
            path_state.has_pending_pen_create(),
            "stash must survive for the real echo"
        );
        assert!(path_state.editing_track_id.is_none());
    }

    #[test]
    fn pen_flush_skipped_when_echo_has_no_correlation_id() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut ui = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.pending_pen_create = Some(pen_stash("pen-track:t5"));
        nodes::handle_node_created(
            "n2",
            "p1",
            "Imported",
            false,
            None,
            [0.0; 3],
            &mut mgr,
            &nav,
            &mut ui,
            &mut path_state,
            &tx,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(ui.drain_actions().is_empty(), "id-less echo must not flush");
        assert!(path_state.has_pending_pen_create());
        assert!(path_state.editing_track_id.is_none());
    }

    #[test]
    fn node_deleted_prunes_node_and_stops_editing_it() {
        let (tx, _rx) = sender();
        let mut mgr = tree();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        path_state.editing_track_id = Some("n1".into());
        let mut path_asset_cache = crate::verse_manager::PathAssetCache::default();
        let mut path_asset_applied = crate::verse_manager::PathAssetApplied::default();
        nodes::handle_node_deleted(
            "n1",
            "p1",
            &mut mgr,
            &nav,
            &mut path_state,
            &tx,
            &mut path_asset_cache,
            &mut path_asset_applied,
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(mgr.all_nodes().all(|n| n.id != "n1"));
        assert!(!mgr.node_index.contains_key("n1"));
        assert!(path_state.editing_track_id.is_none());
    }

    // --- T4: node rename + cascade descendant count ---

    #[test]
    fn node_renamed_updates_the_tree_display_name() {
        let mut mgr = tree();
        nodes::handle_node_renamed("n1", "Renamed", &mut mgr);
        assert_eq!(
            mgr.all_nodes().find(|n| n.id == "n1").unwrap().name,
            "Renamed"
        );
        // Unknown node: clean no-op.
        nodes::handle_node_renamed("missing", "X", &mut mgr);
        assert!(mgr.all_nodes().all(|n| n.name != "X"));
    }

    #[test]
    fn descendant_count_stashes_into_the_open_node_options_confirm() {
        let mut ui = UiManager::default();
        ui.open_dialog(ActiveDialog::NodeOptions {
            node_id: "n1".into(),
            node_name_buf: "N".into(),
            webpage_url_buf: String::new(),
            pending_delete: true,
            descendant_count: None,
        });
        nodes::handle_node_descendant_count("n1", 4, &mut ui);
        match &ui.active_dialog {
            ActiveDialog::NodeOptions {
                descendant_count, ..
            } => assert_eq!(*descendant_count, Some(4)),
            other => panic!("expected NodeOptions, got {other:?}"),
        }
    }

    #[test]
    fn descendant_count_stashes_into_a_matching_context_menu_target() {
        use crate::dialogs::ContextTarget;
        use crate::node_manager::HitTarget;
        let mut ui = UiManager::default();
        ui.open_dialog(ActiveDialog::ContextMenu {
            screen_pos: [0.0, 0.0],
            world_pos: [0.0; 3],
            target: Some(ContextTarget {
                petal_id: None,
                hit: HitTarget::Node(Entity::from_bits(1)),
                node_id: Some("n1".into()),
                stamp: None,
            }),
            pending_delete: true,
            descendant_count: None,
        });
        nodes::handle_node_descendant_count("n1", 2, &mut ui);
        match &ui.active_dialog {
            ActiveDialog::ContextMenu {
                descendant_count, ..
            } => assert_eq!(*descendant_count, Some(2)),
            other => panic!("expected ContextMenu, got {other:?}"),
        }
    }

    #[test]
    fn descendant_count_for_another_node_or_closed_dialog_is_dropped() {
        // Mismatched node id → the open confirm keeps its unknown state.
        let mut ui = UiManager::default();
        ui.open_dialog(ActiveDialog::NodeOptions {
            node_id: "n1".into(),
            node_name_buf: "N".into(),
            webpage_url_buf: String::new(),
            pending_delete: true,
            descendant_count: None,
        });
        nodes::handle_node_descendant_count("other", 9, &mut ui);
        match &ui.active_dialog {
            ActiveDialog::NodeOptions {
                descendant_count, ..
            } => assert!(descendant_count.is_none(), "stale count must not leak"),
            other => panic!("expected NodeOptions, got {other:?}"),
        }
        // No dialog at all → clean no-op.
        let mut ui = UiManager::default();
        nodes::handle_node_descendant_count("n1", 9, &mut ui);
        assert!(matches!(ui.active_dialog, ActiveDialog::None));
    }

    // --- roles / dialog handlers ---

    #[test]
    fn legacy_verse_invite_does_not_open_an_unrendered_dialog() {
        let mut ui=UiManager::default();
        roles::handle_verse_invite_generated("fractal://invite",&mut ui,1.0);
        assert!(matches!(ui.active_dialog,ActiveDialog::None));
        ui.open_dialog(ActiveDialog::JoinDialog {invite_buf:"retained".into()});
        roles::handle_verse_invite_generated("fractal://late-invite",&mut ui,1.1);
        assert!(matches!(&ui.active_dialog,ActiveDialog::JoinDialog {invite_buf} if invite_buf=="retained"));
    }

    #[test]
    fn local_role_resolved_caches_level() {
        let mut local_role = crate::plugin::LocalUserRole::default();
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        roles::handle_local_role_resolved("VERSE#v1", "editor", &nav, &mut local_role);
        assert_eq!(local_role.role, Some(fe_database::RoleLevel::Editor));

        nav.navigate_to_verse("v2", "Verse 2");
        local_role.role = None;
        roles::handle_local_role_resolved("VERSE#v1", "owner", &nav, &mut local_role);
        assert!(
            local_role.role.is_none(),
            "in-flight old-scope grants must not repopulate the cache"
        );
        roles::handle_local_role_resolved("VERSE#v2", "viewer", &nav, &mut local_role);
        assert_eq!(local_role.role, Some(fe_database::RoleLevel::Viewer));
        nav.active_petal_id = Some("orphan".into());
        local_role.role = None;
        roles::handle_local_role_resolved("VERSE#v2", "owner", &nav, &mut local_role);
        assert!(
            local_role.role.is_none(),
            "incomplete navigation never accepts a role reply"
        );
    }

    // --- query / error routing ---

    type QueryBuffers = (GisPanelState, PathEditorState, InspectorFormState);

    /// Each GUI query sender mints its own id into its own buffer.
    fn buffers_with_ids(gis_id: &str, tracks_id: &str, query_id: &str) -> QueryBuffers {
        let (mut gis, mut path, mut ins) = QueryBuffers::default();
        gis.query_pending = true;
        gis.query_request_id = Some(gis_id.to_string());
        path.tracks_pending = true;
        path.tracks_request_id = Some(tracks_id.to_string());
        ins.query_results
            .begin(query_id.to_string(), "VERSE#v".to_string());
        (gis, path, ins)
    }

    fn track_row(node_id: &str) -> serde_json::Value {
        json!({
            "node_id": node_id,
            "display_name": node_id,
            "position": { "type": "Point", "coordinates": [0.0, 0.0] },
            "elevation": 0.0,
        })
    }

    /// FEUI-QUERY-REPLY-ROUTING: GIS, Paths and Query-tab queries all in
    /// flight, replies arriving in reverse order (and one as a failure) —
    /// each lands in its own buffer, never by flag priority.
    #[test]
    fn interleaved_query_replies_land_in_their_own_buffers() {
        let (mut gis, mut path, mut ins) =
            buffers_with_ids("ui-gis-1", "ui-tracks-2", "ui-query-3");
        let query_rows = [json!({ "tab": "query" })];
        let track_rows = [track_row("track-1")];
        let replies: [(&str, Result<&[serde_json::Value], &str>); 3] = [
            ("ui-query-3", Ok(&query_rows)),
            ("ui-tracks-2", Ok(&track_rows)),
            ("ui-gis-1", Err("gis boom")),
        ];
        for (id, reply) in replies {
            assert!(
                query::route_query_reply(id, reply, &mut gis, &mut path, &mut ins),
                "{id} must be claimed"
            );
        }
        assert_eq!(ins.query_results.rows, query_rows);
        assert!(!ins.query_results.pending());
        ins.query_results.invalidate(Some("other"));
        assert!(ins.query_results.rows.is_empty());
        ins.query_results
            .begin("ui-query-4".into(), "VERSE#v-FRACTAL#f".into());
        ins.query_results
            .complete(&[json!({"n":10}), json!({"n":2})]);
        ins.query_results.sort_by(0);
        assert_eq!(ins.query_results.display_order(), &[1, 0]);
        ins.query_results.sort_by(0);
        assert_eq!(ins.query_results.display_order(), &[0, 1]);
        assert_eq!(
            ins.query_results.rows[0]["n"], 10,
            "sorting preserves source row identity"
        );
        ins.query_results
            .begin("ui-query-5".into(), "VERSE#v".into());
        ins.query_results.fail("bad query");
        assert_eq!(ins.query_results.error.as_deref(), Some("bad query"));
        ins.query_results
            .begin("ui-query-6".into(), "VERSE#v".into());
        ins.query_results.complete(&[]);
        assert!(ins.query_results.rows.is_empty() && ins.query_results.error.is_none());
        ins.query_results.complete(&[
            json!({"n":2}),
            json!({"n":10}),
            json!({"n":"15"}),
            json!({"n":null}),
            json!({"n":true}),
        ]);
        ins.query_results.sort_by(0);
        assert_eq!(ins.query_results.display_order(), &[3, 4, 0, 1, 2]);
        ins.query_results.sort_by(0);
        assert_eq!(ins.query_results.display_order(), &[2, 1, 0, 4, 3]);
        ins.query_results.begin("no-verse".into(), String::new());
        ins.query_results.complete(&[json!({"old":true})]);
        ins.query_results.invalidate(Some("v"));
        assert!(
            ins.query_results.rows.is_empty(),
            "entering a verse clears no-verse results"
        );
        let rows_before = ins.query_results.rows.clone();
        query::handle_query_result(&[json!({"foreign":true})], &mut gis, &mut path);
        assert_eq!(
            ins.query_results.rows, rows_before,
            "unmatched legacy replies never replace dock data"
        );

        assert_eq!(path.tracks.len(), 1);
        assert_eq!(path.tracks[0].node_id, "track-1");
        assert!(!path.tracks_pending && path.tracks_request_id.is_none());
        assert_eq!(gis.last_error.as_deref(), Some("gis boom"));
        assert!(
            gis.results.is_empty(),
            "the Query-tab rows must not reach GIS"
        );
        assert!(!gis.query_pending && gis.query_request_id.is_none());
        gis.sync_petal(Some("p"));
        gis.query_request_id = Some("query-independent".into());
        gis.query_pending = true;
        gis.results = crate::gis::parse_gis_rows(&[track_row("query-only")]);
        gis.annotations.request_id = Some("annotations-1".into());
        assert!(query::route_query_reply("annotations-1", Ok(&[]), &mut gis, &mut path, &mut ins));
        assert!(gis.annotations.loaded && gis.annotations.rows.is_empty());
        assert_eq!(gis.results[0].node_id, "query-only");
        assert!(gis.query_pending);
        gis.annotations.request_id = Some("annotations-2".into());
        assert!(query::route_query_reply("annotations-2", Err("denied"), &mut gis, &mut path, &mut ins));
        assert_eq!(gis.annotations.error.as_deref(), Some("denied"));
        gis.annotations.request_id = Some("annotations-old-petal".into());
        gis.sync_petal(Some("other"));
        assert!(!query::route_query_reply("annotations-old-petal", Ok(&track_rows), &mut gis, &mut path, &mut ins));
        assert!(!gis.annotations.loaded && gis.annotations.rows.is_empty() && gis.results.is_empty());
    }

    /// An API waiter's ULID-correlated reply and a stale GUI id (the user
    /// re-submitted, minting a new one) claim no buffer and touch nothing.
    #[test]
    fn foreign_or_stale_query_ids_claim_nothing() {
        for id in ["01HZXAPIULIDAPIULIDAPIULID", "ui-gis-0"] {
            let (mut gis, mut path, mut ins) =
                buffers_with_ids("ui-gis-1", "ui-tracks-2", "ui-query-3");
            let rows = [json!({ "leak": true })];
            assert!(
                !query::route_query_reply(id, Ok(&rows), &mut gis, &mut path, &mut ins),
                "{id} must not be claimed"
            );
            assert!(gis.query_pending && gis.results.is_empty());
            assert!(path.tracks_pending && path.tracks.is_empty());
            assert!(ins.query_results.pending() && ins.query_results.rows.is_empty());
        }
    }

    /// A bare DB `Error` (some unrelated command failed) no longer clobbers
    /// an in-flight query buffer — query failures arrive as `QueryFailed`.
    #[test]
    fn bare_error_leaves_pending_query_buffers_alone() {
        let (gis, mut path, ins) = buffers_with_ids("ui-gis-1", "ui-tracks-2", "ui-query-3");
        let mut ui = UiManager::default();
        let local_role = crate::plugin::LocalUserRole::default();
        let nav = NavigationManager::default();
        query::handle_error(
            "unrelated failure",
            &mut path,
            &mut ui,
            &local_role,
            &nav,
            0.0,
        );
        assert!(gis.query_pending && gis.last_error.is_none());
        assert!(path.tracks_pending && path.last_error.is_none());
        assert!(ins.query_results.pending() && ins.query_results.rows.is_empty());
    }

    /// X-01: every bare `DbResult::Error` now also surfaces as an error
    /// notice and lands in the status-bar history ring, not just the log.
    /// Exercises `query::handle_error` directly (the exact function
    /// `apply_db_results`'s `DbResult::Error` arm calls) rather than
    /// spinning up a full headless `App` with `VerseManagerPlugin` — the
    /// dispatcher's arm is a one-line forward, so this covers the same
    /// behavior without constructing all 16 of `apply_db_results`'s
    /// `SystemParam`s. (CONTRACT-DRIFT note in the W1-S2 report.)
    #[test]
    fn db_error_reaches_error_notice_and_history() {
        let mut path_state = PathEditorState::default();
        let mut ui = UiManager::default();
        let local_role = crate::plugin::LocalUserRole::default();
        let nav = NavigationManager::default();
        query::handle_error(
            "Set petal terrain failed: permission denied: Editor role required",
            &mut path_state,
            &mut ui,
            &local_role,
            &nav,
            1.0,
        );
        assert!(
            ui.active_notice(1.0).is_some(),
            "the error must become the active (highest-priority) notice"
        );
        assert_eq!(
            ui.error_history().count(),
            1,
            "the error must land in the status-bar history ring"
        );
    }

    // --- property handlers (continue semantics) ---

    #[test]
    fn properties_loaded_preserves_unselected_ui_and_delivers_correlated_api_reply() {
        let mut app = App::new();
        let (db_sender, db_commands) = sender();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .add_message::<DbResult>()
            .init_resource::<VerseManager>()
            .init_resource::<NavigationManager>()
            .insert_resource(db_sender)
            .init_resource::<UiManager>()
            .init_resource::<crate::plugin::LocalUserRole>()
            .init_resource::<InspectorFormState>()
            .init_resource::<NodeManager>()
            .init_resource::<PathEditorState>()
            .init_resource::<crate::terrain_map::PetalMapState>()
            .init_resource::<crate::gis::GisPanelState>()
            .init_resource::<fe_runtime::app::PendingApiRequests>()
            .init_resource::<super::super::PrimitiveDescriptorCache>()
            .init_resource::<super::super::PathAssetCache>()
            .init_resource::<super::super::PathAssetApplied>()
            .init_resource::<crate::actions::asset::StampInteractionState>()
            .init_resource::<crate::actions::terrain_proposal::EarthworkNodeMap>()
            .init_resource::<crate::actions::dimension_callout::DimensionCalloutMap>()
            .init_resource::<crate::terrain_proposal_state::ProposalEditState>()
            .init_resource::<crate::timeseries_settings::TimeseriesSettingsState>()
            .init_resource::<crate::plugin::MeshInstanceBudget>()
            .init_resource::<super::super::ResidencyLedger>()
            .init_resource::<crate::visibility::VisibilityState>()
            .init_resource::<crate::notice::NoticeClock>()
            .add_systems(Update, apply_db_results);
        app.world_mut()
            .resource_mut::<InspectorFormState>()
            .node_properties = json!({"existing":"untouched"});
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        app.world_mut()
            .resource_mut::<fe_runtime::app::PendingApiRequests>()
            .enqueue_for(
                &DbCommand::GetNodeProperties {
                    node_id: "n1".into(),
                    correlation_id: Some("api-properties".into()),
                },
                reply_tx,
            );
        for id in [None, Some("wrong")] {
            app.world_mut()
                .write_message(DbResult::NodePropertiesLoaded {
                    node_id: "n1".into(),
                    properties: json!({"api":99}),
                    correlation_id: id.map(str::to_string),
                });
        }
        app.update();
        assert!(
            reply_rx.try_recv().is_err(),
            "GUI and unmatched replies cannot satisfy the API waiter"
        );
        let reply = DbResult::NodePropertiesLoaded {
            node_id: "n1".into(),
            properties: json!({"api":10}),
            correlation_id: Some("api-properties".into()),
        };
        app.world_mut().write_message(reply.clone());
        app.update();
        assert!(
            matches!(reply_rx.try_recv(), Ok(DbResult::NodePropertiesLoaded {
                            node_id, correlation_id: Some(id), properties,
                        }) if node_id == "n1" && id == "api-properties" && properties == json!({"api":10}))
        );
        let (next_tx, mut next_rx) = tokio::sync::oneshot::channel();
        app.world_mut()
            .resource_mut::<fe_runtime::app::PendingApiRequests>()
            .enqueue_for(
                &DbCommand::GetNodeProperties {
                    node_id: "n1".into(),
                    correlation_id: Some("next-properties".into()),
                },
                next_tx,
            );
        app.world_mut().write_message(reply);
        app.update();
        assert!(
            next_rx.try_recv().is_err(),
            "a duplicate cannot claim a new waiter"
        );
        assert_eq!(
            app.world().resource::<InspectorFormState>().node_properties,
            json!({"existing":"untouched"})
        );
        assert!(app.world().resource::<NodeManager>().selected.is_none());

        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<bevy::scene::Scene>()
            .init_resource::<super::super::TextureRegistryRes>()
            .init_resource::<super::super::PrimitiveMaterialAssets>()
            .init_resource::<crate::settings::AppSettings>()
            .init_resource::<crate::settings::PetalRenderOverride>()
            .init_resource::<super::super::StampRenderIndex>()
            .init_resource::<fe_runtime::diag15m::RenderPressure>()
            .add_systems(
                Update,
                (
                    super::super::residency_ledger::update_residency_ledger,
                    super::super::petal_respawn::respawn_on_petal_change,
                    super::super::residency_ledger::apply_node_residency,
                )
                    .chain()
                    .after(apply_db_results),
            );
        app.insert_resource(tree());
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal_in_fractal("v1", "Verse", "f1", "Fractal", "p1");
        app.world_mut()
            .resource_mut::<crate::plugin::LocalUserRole>()
            .role = Some(fe_policy::RoleLevel::Editor);
        app.world_mut().spawn((
            fe_renderer::camera::OrbitCameraController::default(),
            Transform::default(),
            GlobalTransform::default(),
        ));
        let rotation = Quat::from_euler(EulerRot::XYZ, 0.25, 0.5, 0.75).to_array();
        let birth = fe_runtime::messages::CommittedNodeMaterialization {
            node: fe_runtime::messages::NodeDto {
                node_id: "placed".into(),
                petal_id: "p1".into(),
                name: "Placed".into(),
                position: [3.0, 2.0, 4.0],
                rotation,
                scale: [2.0, 0.5, 3.0],
                has_asset: true,
                asset_path: Some("models/placed.glb".into()),
            },
            rotation_euler: [0.25, 0.5, 0.75],
            asset_id: Some("placed-asset".into()),
            properties: json!({}),
            history_version: 1,
        };
        let placed = DbResult::GltfImported {
            request: None,
            node_id: "placed".into(),
            asset_id: "placed-asset".into(),
            petal_id: "p1".into(),
            name: "Placed".into(),
            asset_path: "models/placed.glb".into(),
            position: [3.0, 2.0, 4.0],
            materialization: Some(birth),
            correlation_id: Some("placement".into()),
        };
        let (placed_tx, mut placed_rx) = tokio::sync::oneshot::channel();
        app.world_mut()
            .resource_mut::<fe_runtime::app::PendingApiRequests>()
            .enqueue_for(
                &DbCommand::CreateNodeWithAsset {
                    petal_id: "p1".into(),
                    name: "Placed".into(),
                    asset_id: "placed-asset".into(),
                    position: [3.0, 2.0, 4.0],
                    rotation: [0.25, 0.5, 0.75],
                    scale: [2.0, 0.5, 3.0],
                    correlation_id: Some("placement".into()),
                },
                placed_tx,
            );
        {
            let mut ui = app.world_mut().resource_mut::<UiManager>();
            ui.open_dialog(ActiveDialog::GltfImport {
                file_path_buf: "native.glb".into(),
                name_buf: "Native pending".into(),
                position: [0.0; 3],
            });
            ui.gltf_import_request.start(
                fe_runtime::messages::NativeImportRequest {
                    request_id: "native-current".into(),
                    verse_id: "v1".into(),
                    fractal_id: "f1".into(),
                    petal_id: "p1".into(),
                },
                0.0,
            );
        }
        app.world_mut().write_message(placed.clone());
        app.world_mut().write_message(placed.clone());
        app.update();
        assert!(
            matches!(placed_rx.try_recv(),Ok(DbResult::GltfImported {materialization:Some(_),correlation_id:Some(id),..}) if id=="placement")
        );
        assert!(
            app.world()
                .resource::<UiManager>()
                .gltf_import_request
                .pending
        );
        assert!(
            matches!(
                app.world().resource::<UiManager>().active_dialog,
                ActiveDialog::GltfImport { .. }
            ),
            "an API placement cannot complete the native import dialog's request"
        );
        let expected = Transform {
            translation: Vec3::new(3.0, 2.0, 4.0),
            rotation: Quat::from_array(rotation),
            scale: Vec3::new(2.0, 0.5, 3.0),
        };
        let placed_entry = app
            .world()
            .resource::<VerseManager>()
            .all_nodes()
            .find(|node| node.id == "placed")
            .unwrap();
        assert_eq!(placed_entry.world_transform(), expected);
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .all_nodes()
                .filter(|node| node.id == "placed")
                .count(),
            1
        );
        let placed_entities: Vec<_> = app
            .world_mut()
            .query::<(Entity, &crate::plugin::SpawnedNodeMarker, &Transform)>()
            .iter(app.world())
            .filter(|(_, marker, _)| marker.node_id == "placed")
            .map(|(entity, _, transform)| (entity, *transform))
            .collect();
        assert_eq!(
            placed_entities.len(),
            1,
            "duplicate birth receipts cannot double-spawn a scene"
        );
        assert_eq!(
            placed_entities[0].1, expected,
            "the actual residency spawner receives initial rotation and nonuniform scale"
        );
        let entity = placed_entities[0].0;
        let current:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(json!({
                            "node_id":"placed","petal_id":"p1","display_name":"Placed","asset_id":"placed-asset","position":[9.0,2.0,4.0],"elevation":2.0,
                            "rotation":[0.25,0.5,0.75],"scale":[2.0,0.5,3.0],"interactive":true,"created_at":"test","edit_seq":1,"properties":{}
                        })).unwrap();
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "placed".into(),
                    history_version: 2,
                    state: current,
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: None,
            });
        app.world_mut().write_message(placed.clone());
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(9.0, 2.0, 4.0)
        );
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .committed_versions
                .get("placed"),
            Some(&2)
        );
        let _ = db_commands.try_iter().collect::<Vec<_>>();
        let mut unsupported = placed.clone();
        if let DbResult::GltfImported {
            materialization, ..
        } = &mut unsupported
        {
            *materialization = None;
        }
        let mut mismatched = placed;
        if let DbResult::GltfImported {
            materialization: Some(current),
            ..
        } = &mut mismatched
        {
            current.node.node_id = "foreign-birth".into();
        }
        let admission = app
            .world()
            .resource::<fe_runtime::app::DbCommandSender>()
            .0
            .clone();
        for _ in 0..8 {
            admission
                .try_send(DbCommand::GetNodeProperties {
                    node_id: "queue-blocker".into(),
                    correlation_id: None,
                })
                .unwrap();
        }
        app.world_mut().write_message(unsupported);
        app.world_mut().write_message(mismatched);
        app.update();
        let blockers: Vec<_> = db_commands.try_iter().collect();
        assert_eq!(
            blockers.len(),
            8,
            "a full bounded queue does not block result processing"
        );
        assert!(blockers.iter().all(|command|matches!(command,DbCommand::GetNodeProperties {node_id,..} if node_id=="queue-blocker")));
        app.update();
        app.update();

        assert_eq!(
            db_commands
                .try_iter()
                .filter(|command| matches!(command, DbCommand::LoadHierarchy))
                .count(),
            1
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(9.0, 2.0, 4.0)
        );
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .all_nodes()
                .filter(|node| node.id == "placed")
                .count(),
            1
        );
        assert!(
            app.world()
                .resource::<UiManager>()
                .gltf_import_request
                .pending,
            "an unqualified legacy receipt cannot invent native import completion"
        );
        let previous = fe_runtime::messages::NativeImportRequest {
            request_id: "native-current".into(),
            verse_id: "v1".into(),
            fractal_id: "f1".into(),
            petal_id: "p1".into(),
        };
        let replacement = fe_runtime::messages::NativeImportRequest {
            request_id: "native-replacement".into(),
            ..previous.clone()
        };
        {
            let mut ui = app.world_mut().resource_mut::<UiManager>();
            ui.open_dialog(ActiveDialog::GltfImport {
                file_path_buf: "replacement.glb".into(),
                name_buf: "Replacement".into(),
                position: [0.0; 3],
            });
            ui.gltf_import_request.start(replacement.clone(), 1.0);
            ui.gltf_import_request.expire(32.0, 30.0);
        }
        let mut late = DbResult::GltfImported {
            request: Some(previous.clone()),
            node_id: "late-import".into(),
            asset_id: "placed-asset".into(),
            petal_id: "p1".into(),
            name: "Late import".into(),
            asset_path: "models/placed.glb".into(),
            position: [3.0, 2.0, 4.0],
            materialization: Some(fe_runtime::messages::CommittedNodeMaterialization {
                node: fe_runtime::messages::NodeDto {
                    node_id: "late-import".into(),
                    petal_id: "p1".into(),
                    name: "Late import".into(),
                    position: [3.0, 2.0, 4.0],
                    rotation,
                    scale: [2.0, 0.5, 3.0],
                    has_asset: true,
                    asset_path: Some("models/placed.glb".into()),
                },
                rotation_euler: [0.25, 0.5, 0.75],
                asset_id: Some("placed-asset".into()),
                properties: json!({}),
                history_version: 1,
            }),
            correlation_id: None,
        };
        app.world_mut().write_message(late.clone());
        app.world_mut().write_message(DbResult::ImportGltfFailed {
            request: Some(previous),
            message: "GLTF import failed: stale request".into(),
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .all_nodes()
                .filter(|node| node.id == "late-import")
                .count(),
            1,
            "the committed old import still hydrates the authoritative tree"
        );
        let ui = app.world().resource::<UiManager>();
        assert!(ui.gltf_import_request.pending && ui.gltf_import_request.outcome_unknown);
        assert!(
            ui.gltf_import_request
                .error
                .as_deref()
                .unwrap()
                .contains("unknown")
        );
        assert!(
            matches!(&ui.active_dialog,ActiveDialog::GltfImport {name_buf,..} if name_buf=="Replacement")
        );
        assert_eq!(
            app.world()
                .resource::<NavigationManager>()
                .active_petal_id
                .as_deref(),
            Some("p1")
        );
        if let DbResult::GltfImported { request, .. } = &mut late {
            *request = Some(replacement);
        }
        app.world_mut().write_message(late.clone());
        app.update();
        assert!(
            matches!(
                app.world().resource::<UiManager>().active_dialog,
                ActiveDialog::None
            ),
            "a late but matching success resolves the unknown outcome without another import"
        );
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .all_nodes()
                .filter(|node| node.id == "late-import")
                .count(),
            1
        );
        use fe_runtime::messages::{
            FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
        };
        let reload = DbResult::HierarchyLoaded {
            verses: vec![VerseHierarchyData {
                id: "v1".into(),
                name: "Verse".into(),
                namespace_id: None,
                timeseries: Default::default(),
                view_hidden: false,
                fractals: vec![FractalHierarchyData {
                    id: "f1".into(),
                    name: "Fractal".into(),
                    view_hidden: false,
                    petals: vec![PetalHierarchyData {
                        id: "p1".into(),
                        name: "Petal".into(),
                        view_hidden: false,
                        render_distance: None,
                        nodes: vec![NodeHierarchyData {
                            id: "late-import".into(),
                            name: "Late import".into(),
                            petal_id: "p1".into(),
                            position: [3.0, 2.0, 4.0],
                            rotation,
                            scale: [2.0, 0.5, 3.0],
                            has_asset: true,
                            asset_path: Some("models/placed.glb".into()),
                            webpage_url: None,
                        }],
                    }],
                }],
            }],
        };
        let mut hydration = late.clone();
        if let DbResult::GltfImported {
            materialization, ..
        } = &mut hydration
        {
            *materialization = None;
        }
        app.insert_resource(NavigationManager::default());
        app.world_mut().write_message(reload.clone());
        app.update();
        assert_eq!(
            app.world()
                .resource::<NavigationManager>()
                .active_scope()
                .as_deref(),
            Some("VERSE#v1-FRACTAL#f1-PETAL#p1"),
            "startup still selects the populated Petal when no intent or hydration intervened"
        );
        let _ = db_commands.try_iter().collect::<Vec<_>>();
        app.world_mut().write_message(hydration.clone());
        app.update();
        assert!(
            db_commands
                .try_iter()
                .any(|command| matches!(command, DbCommand::LoadHierarchy))
        );
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .back_from_verse();
        app.world_mut().write_message(reload.clone());
        app.update();
        assert!(
            app.world()
                .resource::<NavigationManager>()
                .active_verse_id
                .is_none(),
            "exit between refresh admission and its late hierarchy reply remains authoritative"
        );
        app.world_mut().write_message(late);
        app.world_mut().write_message(hydration.clone());
        app.update();
        assert!(
            app.world()
                .resource::<NavigationManager>()
                .active_verse_id
                .is_none(),
            "a stale import after explicit exit cannot choose a Verse"
        );
        let _ = db_commands.try_iter().collect::<Vec<_>>();
        app.world_mut().write_message(reload.clone());
        app.world_mut().write_message(reload.clone());
        app.update();
        assert!(
            app.world()
                .resource::<NavigationManager>()
                .active_verse_id
                .is_none(),
            "repeated reloads preserve exit"
        );
        app.insert_resource(NavigationManager::default());
        app.world_mut().write_message(hydration);
        app.update();
        assert!(
            !app.world()
                .resource::<NavigationManager>()
                .initial_auto_navigation_allowed(),
            "receipt-triggered hydration suppresses startup selection without choosing a cursor"
        );
        app.world_mut().write_message(reload);
        app.update();
        assert!(
            app.world()
                .resource::<NavigationManager>()
                .active_verse_id
                .is_none()
        );
    }

    #[test]
    fn properties_loaded_feeds_primitive_cache_even_when_unselected() {
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let mut pa_cache = super::super::PathAssetCache::default();
        let props = json!({"primitive": {"kind": "cube", "dims": [1.0, 1.0, 1.0]}});
        let delivered = properties::handle_node_properties_loaded(
            "n1",
            None,
            &props,
            &mut path_state,
            &NodeManager::default(),
            &mut ins,
            &mut cache,
            &mut pa_cache,
            &mut crate::actions::asset::StampInteractionState::default(),
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::visibility::VisibilityState::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(!delivered, "selection gate unchanged");
        assert!(
            cache.get("n1").is_some(),
            "FR-1: cache fed without selection"
        );
    }

    #[test]
    fn properties_loaded_hydrates_a_persisted_hide_even_when_unselected() {
        let mut visibility = crate::visibility::VisibilityState::default();
        properties::handle_node_properties_loaded(
            "n1",
            Some("p1"),
            &json!({ "view.state": "hide" }),
            &mut PathEditorState::default(),
            &NodeManager::default(),
            &mut InspectorFormState::default(),
            &mut super::super::PrimitiveDescriptorCache::default(),
            &mut super::super::PathAssetCache::default(),
            &mut crate::actions::asset::StampInteractionState::default(),
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut visibility,
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert_eq!(
            visibility.node_override("n1"),
            crate::visibility::OverrideState::Hide
        );
    }

    #[test]
    fn properties_loaded_feeds_path_asset_cache_when_track_has_stamp() {
        // FR-1 path-asset: a track carrying both `path_asset` and `gpx_points`
        // feeds the stamp cache without any selection, tagged with its petal.
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut prim = super::super::PrimitiveDescriptorCache::default();
        let mut pa = super::super::PathAssetCache::default();
        let props = json!({
            "path_asset": {
                "asset_path": "blob://tree.glb",
                "spacing_mode": "fixed_spacing",
                "spacing_value": 5.0
            },
            "gpx_points": [[0.0, 0.0, 0.0, 0.0], [10.0, 0.0, 0.0, 1.0]]
        });
        let delivered = properties::handle_node_properties_loaded(
            "t1",
            Some("p1"),
            &props,
            &mut path_state,
            &NodeManager::default(),
            &mut ins,
            &mut prim,
            &mut pa,
            &mut crate::actions::asset::StampInteractionState::default(),
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::visibility::VisibilityState::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        assert!(!delivered, "selection gate unchanged");
        assert!(
            pa.get("t1").is_some(),
            "FR-1: path-asset cache fed without selection"
        );
    }

    #[test]
    fn properties_loaded_hydrates_promoted_stamp_state() {
        // T2 hydration (3d): a promoted stamp node's property bag re-binds its
        // node id + persisted overrides on reload — independent of selection.
        use crate::actions::asset::{StampInteractionState, StampRef};
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut prim = super::super::PrimitiveDescriptorCache::default();
        let mut pa = super::super::PathAssetCache::default();
        let mut stamps = StampInteractionState::default();
        let props = json!({
            "node_kind": "stamp",
            "path_id": "path-1",
            "instance_index": 4,
            "stamp.override.scale": [2.0, 2.0, 2.0]
        });
        properties::handle_node_properties_loaded(
            "path-1#inst-4",
            Some("p1"),
            &props,
            &mut path_state,
            &NodeManager::default(),
            &mut ins,
            &mut prim,
            &mut pa,
            &mut stamps,
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::visibility::VisibilityState::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
        );
        let stamp = StampRef {
            track_node_id: "path-1".into(),
            stamp_index: 4,
        };
        assert_eq!(stamps.promoted_node_id(&stamp), Some("path-1#inst-4"));
        assert_eq!(
            stamps.override_for(&stamp).and_then(|o| o.scale),
            Some([2.0, 2.0, 2.0]),
            "persisted override survives restart via hydration"
        );
    }

    #[test]
    fn primitive_property_set_invalidates_cache_and_issues_readback_unselected() {
        let (tx, rx) = sender();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        cache.note_properties(
            "n1",
            &json!({"primitive": {"kind": "sphere", "dims": [1.0]}}),
        );
        let for_selected = properties::handle_node_property_set(
            "n1",
            "primitive",
            &nav,
            &tx,
            &mut path_state,
            &NodeManager::default(),
            &mut ins,
            &mut cache,
        );
        assert!(!for_selected);
        assert!(
            cache.get("n1").is_none(),
            "stale descriptor must be evicted"
        );
        assert!(
            matches!(rx.try_recv(), Ok(DbCommand::GetNodeProperties { node_id , correlation_id: None }) if node_id == "n1"),
            "primitive write must re-fetch even when unselected"
        );
    }

    #[test]
    fn primitive_property_deleted_evicts_cache_even_when_unselected() {
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        cache.note_properties(
            "n1",
            &json!({"primitive": {"kind": "sphere", "dims": [1.0]}}),
        );
        let mut pa_cache = super::super::PathAssetCache::default();
        let delivered = properties::handle_node_property_deleted(
            "n1",
            "primitive",
            &NodeManager::default(),
            &mut ins,
            &mut cache,
            &mut pa_cache,
        );
        assert!(!delivered);
        assert!(
            cache.get("n1").is_none(),
            "delete must evict before the selection gate"
        );
    }

    #[test]
    fn properties_loaded_populates_inspector_for_selected_node() {
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let props = json!({"gis.annotation.title": "T", "gis.annotation.body": "B"});
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let mut pa_cache = super::super::PathAssetCache::default();
        let delivered = properties::handle_node_properties_loaded(
            "n1",
            None,
            &props,
            &mut path_state,
            &mgr_selected("n1"),
            &mut ins,
            &mut cache,
            &mut pa_cache,
            &mut crate::actions::asset::StampInteractionState::default(),
            &mut crate::actions::terrain_proposal::EarthworkNodeMap::default(),
            &mut crate::visibility::VisibilityState::default(),
            &mut crate::actions::dimension_callout::DimensionCalloutMap::default(),
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
            "n1",
            "some.key",
            &nav,
            &tx,
            &mut path_state,
            &mgr_selected("n1"),
            &mut ins,
            &mut cache,
        );
        assert!(for_selected);
        assert!(ins.node_properties_loading);
        assert!(
            matches!(rx.try_recv(), Ok(DbCommand::GetNodeProperties { node_id , correlation_id: None }) if node_id == "n1")
        );
    }

    #[test]
    fn property_set_unselected_skips_readback_and_delivery() {
        let (tx, rx) = sender();
        let nav = NavigationManager::default();
        let mut path_state = PathEditorState::default();
        let mut ins = InspectorFormState::default();
        let mut cache = super::super::PrimitiveDescriptorCache::default();
        let for_selected = properties::handle_node_property_set(
            "n1",
            "some.key",
            &nav,
            &tx,
            &mut path_state,
            &NodeManager::default(),
            &mut ins,
            &mut cache,
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
        let mut pa_cache = super::super::PathAssetCache::default();
        let delivered = properties::handle_node_property_deleted(
            "n1",
            "gis.annotation.title",
            &mgr_selected("n1"),
            &mut ins,
            &mut cache,
            &mut pa_cache,
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
        let mut props = crate::terrain_proposal_state::ProposalEditState::default();
        let terrain = Some(json!({"world_scale": 2.5, "tileset_hexon_uris": ["hexon://a"]}));
        // Inactive petal → untouched.
        terrain::handle_petal_terrain_loaded("p1", &terrain, &nav, &mut map, &mut props, None);
        assert!(!map.loaded);
        // Active petal → parsed.
        nav.active_petal_id = Some("p1".into());
        terrain::handle_petal_terrain_loaded("p1", &terrain, &nav, &mut map, &mut props, None);
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
            client: None,
        }];
        tokens::handle_api_tokens_listed(&infos, 1, &mut ui, &mut ins);
        assert_eq!(ins.api_tokens.len(), 1);
        assert_eq!(ins.api_tokens[0].jti, "j1");
        assert_eq!(ins.api_tokens_total, 1);
        assert!(!ins.api_tokens_loading);
    }
}
