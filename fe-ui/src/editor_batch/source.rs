//! Live path draft and rendering fences; see editor_batch/AGENTS.md.

use crate::{
    gis::PathEditorState,
    node_manager::{NodeManager, TrackPickShape},
    verse_manager::PathAssetCache,
};
use bevy::prelude::*;
use fe_runtime::editor_batch::{EditorBatchCapture, EditorBatchEdit, EditorBatchMember};

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SelectionSources<'w, 's> {
    shapes: Query<'w, 's, &'static TrackPickShape>,
    cache: Option<Res<'w, PathAssetCache>>,
}

pub(super) const CLOSED_PATH_REASON: &str = "The rendered source for closed paths changed or is unavailable. Refresh the path before editing; nothing was submitted.";

pub(super) fn closed_path(member: &EditorBatchMember) -> bool {
    member.stamp_path.is_none()
        && member
            .state
            .properties
            .get("gis.track.closed")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

type ReplayTargets<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::plugin::SpawnedNodeMarker,
        &'static Transform,
        Option<&'static Visibility>,
        Option<&'static crate::verse_manager::PathAssetInstance>,
        Option<&'static crate::verse_manager::StampSelectionSource>,
    ),
>;

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RenderedSources<'w, 's> {
    shapes: Query<'w, 's, &'static TrackPickShape>,
    live: ReplayTargets<'w, 's>,
}

impl RenderedSources<'_, '_> {
    pub(crate) fn validate_replay(
        &self,
        capture: &EditorBatchCapture,
        path: &PathEditorState,
        cache: &PathAssetCache,
        stamps: &crate::actions::asset::StampInteractionState,
        nav: &crate::navigation_manager::NavigationManager,
        visibility: &crate::visibility::VisibilityState,
    ) -> Result<(), String> {
        if nav.active_petal_id.as_deref() != Some(capture.petal_id.as_str()) {
            return Err("Undo targets belong to another Petal; no inverse was sent".into());
        }
        let group_registry_edit = !matches!(
            &capture.target,
            fe_runtime::editor_batch::EditorBatchTarget::Nodes
        );
        let mut entities = Vec::with_capacity(capture.members.len());
        for member in &capture.members {
            let stamp = stamps.promoted_stamp_for_node(&member.node_id);
            let mut candidates =
                self.live
                    .iter()
                    .filter(|(entity, marker, _, drawn, instance, _)| {
                        marker.petal_id == capture.petal_id
                            && (marker.node_id == member.node_id
                                || stamp.is_some_and(|stamp| {
                                    crate::verse_manager::parse_stamp_marker_id(&marker.node_id)
                                        == Some((stamp.track_node_id.as_str(), stamp.stamp_index))
                                }))
                            && (group_registry_edit
                                || (crate::visibility::rendered(*drawn)
                                    && crate::visibility::pickable(
                                        &member.node_id,
                                        nav,
                                        visibility,
                                    )
                                    && crate::visibility::pickable(
                                        &marker.node_id,
                                        nav,
                                        visibility,
                                    )))
                            && (member.stamp_path.is_some() || instance.is_none())
                            && (member.stamp_path.is_some()
                                || !member.state.properties.contains_key("gpx_points")
                                || self.shapes.contains(*entity))
                    });
            let Some((entity, _, actual, _, instance, proof)) = candidates.next() else {
                return Err(format!("Undo target {} is not currently available in its owning Petal; no inverse was sent", member.node_id));
            };
            if candidates.next().is_some() || entities.contains(&entity) {
                return Err(
                    "Undo targets have ambiguous rendered identities; no inverse was sent".into(),
                );
            }
            entities.push(entity);
            if let Some(instance) = instance {
                let Some(owner) = &member.stamp_path else {
                    return Err("Undo stamp source identity changed".into());
                };
                let cached = cache.stamp_selection_source(&owner.state.node_id, &capture.petal_id);
                if instance.source_track_id != owner.state.node_id
                    || instance.petal_id != capture.petal_id
                    || !cached
                        .zip(proof)
                        .is_some_and(|((_, descriptor, points), proof)| {
                            proof.0.descriptor == *descriptor
                                && proof.0.points.as_slice() == points
                                && proof.0.world_scale == capture.world_scale
                        })
                {
                    return Err(
                        "Undo stamp render source changed or is unavailable; no inverse was sent"
                            .into(),
                    );
                }
            }
            let expected = super::math::member_transform(member, capture.world_scale)?;
            if !crate::undo::transform_value(&expected)
                .matches(&crate::undo::transform_value(actual))
                || !member_matches(
                    member,
                    &capture.petal_id,
                    path,
                    Some(cache),
                    self.shapes.get(entity).ok(),
                )
            {
                return Err(format!("Undo target {} has an unsaved draft or changed rendered source; no inverse was sent", member.node_id));
            }
        }
        Ok(())
    }
}

impl SelectionSources<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &self,
        state: &mut super::EditorBatchState,
        origin: super::BatchOrigin,
        edit: EditorBatchEdit,
        nav: &crate::navigation_manager::NavigationManager,
        nodes: &NodeManager,
        path: &PathEditorState,
        db: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    ) -> Result<(), String> {
        self.validate(&origin.capture, nodes, path)?;
        state.start(origin, edit, nav, nodes, db)
    }

    pub(crate) fn validate(
        &self,
        capture: &EditorBatchCapture,
        nodes: &NodeManager,
        path: &PathEditorState,
    ) -> Result<(), String> {
        if capture.members.len() != nodes.selection_count() {
            return Err("Selection changed before source validation".into());
        }
        for (captured, selected) in capture.members.iter().zip(nodes.members()) {
            if captured.node_id != selected.node_id {
                return Err("Selection changed before source validation".into());
            }
            if !self.matches(captured, selected.entity, &capture.petal_id, path) {
                if closed_path(captured)
                    || self
                        .shapes
                        .get(selected.entity)
                        .is_ok_and(|shape| shape.closed)
                {
                    return Err(CLOSED_PATH_REASON.into());
                }
                return Err(format!("Selected path or stamp {} has an unsaved draft or changed/unavailable rendered source. Finish or discard that draft before applying the complete selection; nothing was sent.", captured.node_id));
            }
        }
        Ok(())
    }

    pub(crate) fn matches(
        &self,
        captured: &EditorBatchMember,
        entity: Entity,
        petal: &str,
        path: &PathEditorState,
    ) -> bool {
        member_matches(
            captured,
            petal,
            path,
            self.cache.as_deref(),
            self.shapes.get(entity).ok(),
        )
    }
}

fn member_matches(
    captured: &EditorBatchMember,
    petal: &str,
    path: &PathEditorState,
    cache: Option<&PathAssetCache>,
    shape: Option<&TrackPickShape>,
) -> bool {
    let stored_path = captured
        .stamp_path
        .as_ref()
        .map(|path| &path.state)
        .unwrap_or(&captured.state);
    let Some(points) = stored_path.properties.get("gpx_points") else {
        return captured.stamp_path.is_none();
    };
    let Some(anchors) = fe_sdk::path_geometry::decode_anchor_rows(points) else {
        return false;
    };
    if path.editing_track_id.as_deref() == Some(stored_path.node_id.as_str()) {
        let mut stored = crate::gis::decode_gpx_points(points);
        let mut draft = path.points.clone();
        for row in stored.iter_mut().chain(&mut draft) {
            row.time_seconds = Some(row.time_seconds.unwrap_or(0.0));
        }
        if stored != draft {
            return false;
        }
        let closed = stored_path
            .properties
            .get("gis.track.closed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let width = crate::gis::TrackStyleFields::width_from_property(
            stored_path.properties.get("gis.track.width"),
        );
        if path.edited_track_style.closed != closed || path.edited_track_style.width != width {
            return false;
        }
    }
    if captured.stamp_path.is_some() {
        let expected = fe_sdk::path_geometry::flatten_anchor_path(
            &anchors,
            fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
        );
        return cache
            .and_then(|cache| cache.stamp_selection_source(&stored_path.node_id, petal))
            .is_some_and(|(properties, _, rendered)| {
                properties == &serde_json::Value::Object(stored_path.properties.clone())
                    && rendered == expected
            });
    }
    let Some(shape) = shape else {
        return false;
    };
    if shape.anchors.len() != anchors.len()
        || !shape
            .anchors
            .iter()
            .zip(&anchors)
            .all(|(actual, expected)| actual.abs_diff_eq(Vec3::from_array(expected.0), 0.0001))
    {
        return false;
    }
    if shape.closed || closed_path(captured) {
        let width = crate::gis::TrackStyleFields::width_from_property(
            captured.state.properties.get("gis.track.width"),
        );
        return shape.closed == closed_path(captured)
            && shape.half_width == width.max(0.01) / 2.0
            && shape.matches_render_source(points);
    }
    let expected = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    shape.points.len() == expected.len()
        && shape
            .points
            .iter()
            .zip(&expected)
            .all(|(actual, expected)| actual.abs_diff_eq(Vec3::from_array(*expected), 0.0001))
}
