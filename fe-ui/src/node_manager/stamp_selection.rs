//! Live stamp selection adapter; see node_manager/AGENTS.md section stamp-selection.

use super::{context_pick, inspector_sync, pointer, NodeManager};
use crate::{
    actions::{
        asset::{StampInteractionState, StampRef},
        UiManager,
    },
    gis::PathEditorState,
    navigation_manager::NavigationManager,
    plugin::{LocalUserRole, SpawnedNodeMarker, UiSet},
    terrain_map::PetalMapState,
    verse_manager::{parse_stamp_marker_id, PathAssetCache, PathAssetInstance, VerseManager},
    visibility::{pickable, rendered, VisibilityState},
};
use bevy::prelude::*;
use fe_runtime::{
    app::DbCommandSender,
    messages::{CheckedStampPromotion, DbCommand, DbResult},
};
use std::sync::Arc;

#[derive(Debug)]
struct StampSource {
    capture: CheckedStampPromotion,
    navigation: [String; 3],
    descriptor: fe_sdk::path_asset::PathAssetDescriptor,
    points: Vec<[f32; 3]>,
    route: Option<crate::undo::PointEditOrigin>,
    count: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct StampTarget {
    pub entity: Entity,
    pub node_id: String,
    pub index: usize,
}

/// Captured live scene identity and immutable source shared by selection surfaces.
#[derive(Debug, Clone)]
pub struct StampSelectionOrigin {
    source: Arc<StampSource>,
    target: StampTarget,
    generation: u64,
}
impl StampSelectionOrigin {
    pub(crate) fn is_current_selection(&self, manager: &NodeManager) -> bool {
        manager.selected_entity() == Some(self.target.entity)
            && manager.selection_generation == self.generation
            && manager.pending_sidebar_select.is_none()
            && manager.pending_navigation_select.is_none()
    }
    pub(crate) fn stamp_ref(&self) -> StampRef {
        StampRef {
            track_node_id: self.source.capture.path_id.clone(),
            stamp_index: self.target.index,
        }
    }
    pub(crate) fn is_target(&self, entity: Entity, track: &str, index: usize) -> bool {
        self.target.entity == entity
            && self.source.capture.path_id == track
            && self.target.index == index
    }
    fn promotion(&self) -> CheckedStampPromotion {
        let mut capture = self.source.capture.clone();
        capture.instance_index = self.target.index as u32;
        capture
    }
}

#[derive(Resource, Default, Clone)]
pub(crate) struct StampSelectionCatalog {
    source: Option<Arc<StampSource>>,
    pub(crate) targets: Arc<[StampTarget]>,
    generation: u64,
    revision_key: u64,
}
impl StampSelectionCatalog {
    pub(crate) fn for_path(&self, track: &str) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.capture.path_id == track)
    }
    pub(crate) fn origin(&self, index: usize) -> Option<StampSelectionOrigin> {
        Some(StampSelectionOrigin {
            source: self.source.clone()?,
            target: self
                .targets
                .iter()
                .find(|target| target.index == index)?
                .clone(),
            generation: self.generation,
        })
    }
}

#[derive(Resource, Default)]
pub(crate) struct StampSelectionRequests {
    pending: Vec<StampSelectionOrigin>,
    edits: Vec<(
        StampSelectionOrigin,
        fe_runtime::messages::StampOverrideEdit,
    )>,
}
impl StampSelectionRequests {
    pub(crate) fn push(&mut self, origin: StampSelectionOrigin) {
        self.pending.push(origin);
    }
    pub(crate) fn push_edit(
        &mut self,
        origin: StampSelectionOrigin,
        edit: fe_runtime::messages::StampOverrideEdit,
    ) {
        self.edits.push((origin, edit));
    }
}

type LiveStamps<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static SpawnedNodeMarker,
        Option<&'static PathAssetInstance>,
        Option<&'static Visibility>,
        Option<&'static crate::verse_manager::StampSelectionSource>,
    ),
>;

fn scale(map: &PetalMapState) -> f32 {
    let value = map.world_scale as f32;
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    }
}
fn navigation(nav: &NavigationManager) -> Option<[&str; 3]> {
    Some([
        nav.active_verse_id.as_deref()?,
        nav.active_fractal_id.as_deref()?,
        nav.active_petal_id.as_deref()?,
    ])
}

fn source_matches(
    source: &StampSource,
    nav: &NavigationManager,
    path: &PathEditorState,
    cache: &PathAssetCache,
    hierarchy: &VerseManager,
    map: &PetalMapState,
) -> bool {
    if navigation(nav) != Some(source.navigation.each_ref().map(String::as_str))
        || scale(map) != source.capture.world_scale
        || hierarchy
            .committed_versions
            .get(&source.capture.path_id)
            .copied()
            != source.capture.path_version
        || !hierarchy
            .find_petal(&source.capture.petal_id)
            .is_some_and(|petal| {
                petal
                    .nodes
                    .iter()
                    .any(|node| node.id == source.capture.path_id)
            })
    {
        return false;
    }
    let Some((properties, descriptor, points)) =
        cache.stamp_selection_source(&source.capture.path_id, &source.capture.petal_id)
    else {
        return false;
    };
    properties == &source.capture.before_properties
        && descriptor == &source.descriptor
        && points == source.points
        && if path.editing_track_id.as_deref() == Some(source.capture.path_id.as_str()) {
            source.route.as_ref().is_some_and(|origin| {
                origin.matches(path, nav)
                    && path.edited_track_path_asset.as_ref() == Some(&source.descriptor)
            })
        } else {
            source.route.is_none()
        }
}

fn source_for(
    track: &str,
    nav: &NavigationManager,
    path: &PathEditorState,
    cache: &PathAssetCache,
    hierarchy: &VerseManager,
    map: &PetalMapState,
) -> Option<Arc<StampSource>> {
    let ids = navigation(nav)?;
    let petal = ids[2];
    if !hierarchy
        .find_petal(petal)?
        .nodes
        .iter()
        .any(|node| node.id == track)
    {
        return None;
    }
    let (properties, descriptor, cached_points) = cache.stamp_selection_source(track, petal)?;
    let stored_descriptor =
        fe_sdk::path_asset::PathAssetDescriptor::from_json(&properties["path_asset"]).ok()?;
    if &stored_descriptor != descriptor || descriptor.asset_path.trim().is_empty() {
        return None;
    }
    let anchors = fe_sdk::path_geometry::decode_anchor_rows(&properties["gpx_points"])?;
    let points = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    if points != cached_points {
        return None;
    }
    let route = if path.editing_track_id.as_deref() == Some(track) {
        if path.edited_track_path_asset.as_ref() != Some(descriptor) {
            return None;
        }
        let live_anchors: Vec<_> = path
            .points
            .iter()
            .map(|row| (row.position, row.handle_in, row.handle_out))
            .collect();
        if fe_sdk::path_geometry::flatten_anchor_path(
            &live_anchors,
            fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
        ) != points
        {
            return None;
        }
        crate::undo::PointEditOrigin::capture(path, nav)
    } else {
        None
    };
    let world_scale = scale(map);
    let total = fe_sdk::path_geometry::polyline_length(&points);
    if !total.is_finite() {
        return None;
    }
    let count =
        fe_sdk::path_asset::sample_progresses(&points, descriptor, total, world_scale).len();
    Some(Arc::new(StampSource {
        capture: CheckedStampPromotion {
            path_id: track.into(),
            petal_id: petal.into(),
            scope: nav.active_scope()?,
            instance_index: 0,
            path_version: hierarchy.committed_versions.get(track).copied(),
            before_properties: properties.clone(),
            world_scale,
        },
        navigation: ids.map(str::to_owned),
        descriptor: descriptor.clone(),
        points,
        route,
        count,
    }))
}

fn target_from<'a>(
    marker: &'a SpawnedNodeMarker,
    instance: Option<&'a PathAssetInstance>,
    state: &'a StampInteractionState,
) -> Option<(&'a str, usize)> {
    if let Some(instance) = instance {
        let (track, index) = parse_stamp_marker_id(&marker.node_id)?;
        return (track == instance.source_track_id && marker.petal_id == instance.petal_id)
            .then_some((track, index));
    }
    let stamp = state.promoted_stamp_for_node(&marker.node_id)?;
    Some((stamp.track_node_id.as_str(), stamp.stamp_index))
}

fn render_source_matches(
    source: &StampSource,
    instance: Option<&PathAssetInstance>,
    proof: Option<&crate::verse_manager::StampSelectionSource>,
) -> bool {
    instance.is_none()
        || proof.is_some_and(|proof| {
            proof.0.descriptor == source.descriptor
                && proof.0.points == source.points
                && proof.0.world_scale == source.capture.world_scale
        })
}

fn origin_live(
    origin: &StampSelectionOrigin,
    live: &LiveStamps,
    state: &StampInteractionState,
    nav: &NavigationManager,
    visibility: &VisibilityState,
) -> bool {
    let Ok((_, marker, instance, drawn, proof)) = live.get(origin.target.entity) else {
        return false;
    };
    marker.node_id == origin.target.node_id
        && marker.petal_id == origin.source.capture.petal_id
        && render_source_matches(&origin.source, instance, proof)
        && origin.target.index < origin.source.count
        && rendered(drawn)
        && pickable(&marker.node_id, nav, visibility)
        && target_from(marker, instance, state).is_some_and(|(track, index)| {
            track == origin.source.capture.path_id && index == origin.target.index
        })
}

/// Capture only on a real pick; lower path-edit priority tiers remain unchanged.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct StampPickParams<'w, 's> {
    nav: Res<'w, NavigationManager>,
    path: Res<'w, PathEditorState>,
    cache: Res<'w, PathAssetCache>,
    hierarchy: Res<'w, VerseManager>,
    map: Res<'w, PetalMapState>,
    state: Res<'w, StampInteractionState>,
    live: LiveStamps<'w, 's>,
    requests: ResMut<'w, StampSelectionRequests>,
}
impl StampPickParams<'_, '_> {
    pub(super) fn is_stamp(&self, entity: Entity) -> bool {
        self.live
            .get(entity)
            .is_ok_and(|(_, marker, instance, _, _)| {
                target_from(marker, instance, &self.state).is_some()
            })
    }
    pub(super) fn queue(&mut self, entity: Entity, generation: u64) {
        let Ok((entity, marker, instance, _, proof)) = self.live.get(entity) else {
            return;
        };
        let Some((track, index)) = target_from(marker, instance, &self.state) else {
            return;
        };
        let Some(source) = source_for(
            &track,
            &self.nav,
            &self.path,
            &self.cache,
            &self.hierarchy,
            &self.map,
        ) else {
            return;
        };
        if index >= source.count
            || marker.petal_id != source.capture.petal_id
            || !render_source_matches(&source, instance, proof)
        {
            return;
        }
        self.requests.push(StampSelectionOrigin {
            source,
            target: StampTarget {
                entity,
                node_id: marker.node_id.clone(),
                index,
            },
            generation,
        });
    }
}

fn select_context_stamp(
    ui: Res<UiManager>,
    manager: Res<NodeManager>,
    mut pick: StampPickParams,
    mut last: Local<Option<Entity>>,
) {
    let entity = match &ui.active_dialog {
        crate::dialogs::ActiveDialog::ContextMenu {
            target: Some(target),
            ..
        } => match &target.hit {
            super::HitTarget::Stamp(entity) | super::HitTarget::Node(entity)
                if pick.is_stamp(*entity) =>
            {
                Some(*entity)
            }
            _ => None,
        },
        _ => None,
    };
    if entity != *last {
        *last = entity;
        if let Some(entity) = entity {
            pick.queue(entity, manager.selection_generation);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_selections(
    mut requests: ResMut<StampSelectionRequests>,
    mut manager: ResMut<NodeManager>,
    mut state: ResMut<StampInteractionState>,
    nav: Res<NavigationManager>,
    mut path: ResMut<PathEditorState>,
    cache: Res<PathAssetCache>,
    hierarchy: Res<VerseManager>,
    map: Res<PetalMapState>,
    live: LiveStamps,
    visibility: Res<VisibilityState>,
    role: Res<LocalUserRole>,
    db: Res<DbCommandSender>,
    mut ui: ResMut<UiManager>,
    time: Res<crate::notice::NoticeClock>,
) {
    for mut origin in std::mem::take(&mut requests.pending) {
        if origin.generation != manager.selection_generation
            || !role.role.is_some_and(|role| role.can_view())
            || !source_matches(&origin.source, &nav, &path, &cache, &hierarchy, &map)
            || !origin_live(&origin, &live, &state, &nav, &visibility)
        {
            state.selection_error("Stamp selection changed; select a current visible stamp again");
            continue;
        }
        let stamp = origin.stamp_ref();
        let capture = origin.promotion();
        let promoted = state
            .promoted_node_id(&stamp)
            .filter(|node_id| state.promoted_stamp_for_node(node_id) == Some(&stamp));
        let node_id = promoted.unwrap_or(&origin.target.node_id).to_owned();
        manager.select_live_stamp(origin.target.entity, node_id.clone());
        if promoted.is_some() {
            manager.bind_promoted_stamp(origin.target.entity, &node_id);
        }
        origin.generation = manager.selection_generation;
        path.selected_point = None;
        path.selected_segment = None;
        if !state.selection_origin.as_ref().is_some_and(|current| {
            current.is_target(
                origin.target.entity,
                &origin.source.capture.path_id,
                origin.target.index,
            )
        }) {
            state.selection_promotion = None;
        }
        state.select_live(origin);
        if !role.can_edit() {
            let feedback = if state.is_promoted(&stamp) {
                "Read-only stamp inspection. Editor access is required to apply changes."
            } else {
                "Read-only stamp selection; no stored node is loaded. Editor access is required for promotion."
            };
            state.selection_error(feedback);
            if let Some(node_id) = state.promoted_node_id(&stamp) {
                if db
                    .0
                    .send(DbCommand::GetNodeProperties {
                        node_id: node_id.into(),
                        correlation_id: None,
                    })
                    .is_err()
                {
                    bevy::log::warn!("stamp inspection read channel closed");
                }
            }
            continue;
        }
        if state.selection_promotion.is_some() {
            continue;
        }
        let request = format!("ui-stamp-promotion-{}", ulid::Ulid::new());
        match db.0.send(DbCommand::PromoteCheckedStamp {
            capture,
            correlation_id: request.clone(),
        }) {
            Ok(()) => state.selection_promotion = Some(request),
            Err(_) => state
                .selection_error("Database connection closed; stamp selection was not promoted"),
        }
    }
    for (origin, edit) in std::mem::take(&mut requests.edits) {
        if !origin.is_current_selection(&manager)
            || !source_matches(&origin.source, &nav, &path, &cache, &hierarchy, &map)
            || !origin_live(&origin, &live, &state, &nav, &visibility)
        {
            ui.show_advisory("Stamp, scene or source changed; select and review the current stamp before applying",time.elapsed_secs_f64());
            continue;
        }
        match crate::actions::asset::apply_overrides(&db, &mut state, edit, &nav, &path, &role) {
            Ok(()) => ui.show_toast("Saving stamp changes...", time.elapsed_secs_f64()),
            Err(reason) => ui.show_error(reason, time.elapsed_secs_f64()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn maintain_selection_and_catalog(
    mut state: ResMut<StampInteractionState>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    path: Res<PathEditorState>,
    cache: Res<PathAssetCache>,
    hierarchy: Res<VerseManager>,
    map: Res<PetalMapState>,
    live: LiveStamps,
    visibility: Res<VisibilityState>,
    mut catalog: ResMut<StampSelectionCatalog>,
    mut tools: ResMut<crate::panels::tool_panel::ToolPanelState>,
    mut requests: ResMut<StampSelectionRequests>,
    mut attempted: Local<Option<(Entity, u64)>>,
) {
    if state.selection_origin.as_ref().is_some_and(|origin| {
        manager.selected_entity() != Some(origin.target.entity)
            || manager.selection_generation != origin.generation
            || path.selected_point.is_some()
            || path.selected_segment.is_some()
            || !source_matches(&origin.source, &nav, &path, &cache, &hierarchy, &map)
            || !origin_live(origin, &live, &state, &nav, &visibility)
    }) {
        *attempted = manager
            .selected_entity()
            .map(|entity| (entity, manager.selection_generation));
        state.clear_selection();
    }
    if let Some(origin) = state.selection_origin.as_ref() {
        if let Some(node_id) = state
            .selected()
            .filter(|stamp| {
                origin.is_target(
                    origin.target.entity,
                    &stamp.track_node_id,
                    stamp.stamp_index,
                )
            })
            .and_then(|stamp| {
                state
                    .promoted_node_id(stamp)
                    .filter(|node_id| state.promoted_stamp_for_node(node_id) == Some(stamp))
            })
        {
            if manager
                .selected
                .as_ref()
                .is_some_and(|selected| selected.node_id != node_id)
            {
                let entity = origin.target.entity;
                let node_id = node_id.to_owned();
                manager.bind_promoted_stamp(entity, &node_id);
                if let Some(origin) = state.selection_origin.as_mut() {
                    origin.generation = manager.selection_generation;
                }
            }
        }
    }
    if state.selection_origin.is_none()
        && path.selected_point.is_none()
        && path.selected_segment.is_none()
    {
        if let Some(selected) = manager.selected.as_ref() {
            let key = (selected.entity, manager.selection_generation);
            if *attempted != Some(key) {
                if let Ok((entity, marker, instance, _, proof)) = live.get(selected.entity) {
                    if let Some((track, index)) = target_from(marker, instance, &state) {
                        if let Some(source) =
                            source_for(track, &nav, &path, &cache, &hierarchy, &map)
                        {
                            if index < source.count
                                && render_source_matches(&source, instance, proof)
                            {
                                requests.push(StampSelectionOrigin {
                                    source,
                                    target: StampTarget {
                                        entity,
                                        node_id: marker.node_id.clone(),
                                        index,
                                    },
                                    generation: manager.selection_generation,
                                });
                                *attempted = Some(key);
                            }
                        }
                    }
                }
            }
        } else {
            *attempted = None;
        }
    }
    let Some(track) = path.editing_track_id.as_deref() else {
        if catalog.source.is_some() {
            *catalog = Default::default();
            tools.stamp_selection_catalog = None;
        }
        return;
    };
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    navigation(&nav).hash(&mut hash);
    track.hash(&mut hash);
    manager.selection_generation.hash(&mut hash);
    cache.selection_revision().hash(&mut hash);
    scale(&map).to_bits().hash(&mut hash);
    hierarchy.committed_versions.get(track).hash(&mut hash);
    hierarchy
        .find_petal(nav.active_petal_id.as_deref().unwrap_or_default())
        .is_some_and(|petal| petal.nodes.iter().any(|node| node.id == track))
        .hash(&mut hash);
    crate::undo::PointEditOrigin::confirmation_key(&path, &nav).hash(&mut hash);
    if let Some(descriptor) = path.edited_track_path_asset.as_ref() {
        descriptor.asset_path.hash(&mut hash);
        descriptor.spacing_value.to_bits().hash(&mut hash);
        descriptor.count.hash(&mut hash);
        descriptor.tangent_align.hash(&mut hash);
        matches!(
            descriptor.spacing_mode,
            fe_sdk::path_asset::SpacingMode::FixedCount
        )
        .hash(&mut hash);
    }
    for (entity, marker, instance, drawn, _) in &live {
        if Some(marker.petal_id.as_str()) == nav.active_petal_id.as_deref()
            && target_from(marker, instance, &state).is_some_and(|(owner, _)| owner == track)
        {
            entity.hash(&mut hash);
            marker.node_id.hash(&mut hash);
            rendered(drawn).hash(&mut hash);
            pickable(&marker.node_id, &nav, &visibility).hash(&mut hash);
        }
    }
    let revision_key = hash.finish();
    if catalog.revision_key == revision_key {
        return;
    }
    let Some(source) = source_for(track, &nav, &path, &cache, &hierarchy, &map) else {
        *catalog = StampSelectionCatalog {
            revision_key,
            ..Default::default()
        };
        tools.stamp_selection_catalog = None;
        return;
    };
    let mut targets: Vec<_> = live
        .iter()
        .filter_map(|(entity, marker, instance, drawn, proof)| {
            if marker.petal_id != source.capture.petal_id
                || !rendered(drawn)
                || !pickable(&marker.node_id, &nav, &visibility)
            {
                return None;
            }
            let (owner, index) = target_from(marker, instance, &state)?;
            (owner == track
                && index < source.count
                && render_source_matches(&source, instance, proof))
            .then(|| StampTarget {
                entity,
                node_id: marker.node_id.clone(),
                index,
            })
        })
        .collect();
    targets.sort_by_key(|target| target.index);
    *catalog = StampSelectionCatalog {
        source: Some(source),
        targets: targets.into(),
        generation: manager.selection_generation,
        revision_key,
    };
    if tools.stamp_edit_index == 0 {
        tools.stamp_edit_index = catalog
            .targets
            .first()
            .map_or(1, |target| target.index.saturating_add(1) as u32);
    }
    tools.stamp_selection_catalog = Some(catalog.clone());
}

#[allow(clippy::too_many_arguments)]
fn receive_selection_results(
    mut replies: MessageReader<DbResult>,
    mut state: ResMut<StampInteractionState>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    path: Res<PathEditorState>,
    cache: Res<PathAssetCache>,
    hierarchy: Res<VerseManager>,
    map: Res<PetalMapState>,
    live: LiveStamps,
    visibility: Res<VisibilityState>,
) {
    for reply in replies.read() {
        match reply {
            DbResult::CheckedStampPromoted {
                capture,
                node_id,
                properties,
                correlation_id,
                ..
            } if state.selection_promotion.as_deref() == Some(correlation_id.as_str()) => {
                let identity = node_id
                    == &format!("{}#inst-{}", capture.path_id, capture.instance_index)
                    && properties["node_kind"] == "stamp"
                    && properties["path_id"] == capture.path_id
                    && properties["owning_path_id"] == capture.path_id
                    && properties["instance_index"].as_u64()
                        == Some(u64::from(capture.instance_index));
                let current = identity
                    && state.selection_origin.as_ref().is_some_and(|origin| {
                        origin.promotion() == *capture
                            && origin.is_current_selection(&manager)
                            && source_matches(&origin.source, &nav, &path, &cache, &hierarchy, &map)
                            && origin_live(origin, &live, &state, &nav, &visibility)
                    });
                state.selection_promotion = None;
                if current {
                    let entity = state.selection_origin.as_ref().unwrap().target.entity;
                    crate::actions::asset::hydrate_promoted_stamp(node_id, properties, &mut state);
                    if manager.bind_promoted_stamp(entity, node_id) {
                        if let Some(origin) = state.selection_origin.as_mut() {
                            origin.generation = manager.selection_generation;
                        }
                    }
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if state.selection_promotion.as_deref() == Some(correlation_id.as_str()) => {
                state.selection_promotion = None;
                state.selection_error(error.clone());
            }
            _ => {}
        }
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<StampSelectionCatalog>()
        .init_resource::<StampSelectionRequests>();
    app.add_systems(
        Update,
        (
            select_context_stamp,
            dispatch_selections,
            maintain_selection_and_catalog,
            receive_selection_results,
        )
            .chain()
            .after(super::viewport_pick::handle_viewport_click)
            .after(context_pick::classify_context_menu)
            .after(pointer::open_track_on_select)
            .before(inspector_sync::sync_manager_to_inspector)
            .in_set(UiSet::Selection),
    );
}
