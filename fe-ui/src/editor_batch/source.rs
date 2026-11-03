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
    let expected = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    if path.editing_track_id.as_deref() == Some(stored_path.node_id.as_str()) {
        let mut stored = crate::gis::decode_gpx_points(points);
        let mut draft = path.points.clone();
        for row in stored.iter_mut().chain(&mut draft) {
            row.time_seconds = Some(row.time_seconds.unwrap_or(0.0));
        }
        if stored != draft {
            return false;
        }
    }
    if captured.stamp_path.is_some() {
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
    shape.closed
        || (shape.points.len() == expected.len()
            && shape
                .points
                .iter()
                .zip(&expected)
                .all(|(actual, expected)| actual.abs_diff_eq(Vec3::from_array(*expected), 0.0001)))
}
