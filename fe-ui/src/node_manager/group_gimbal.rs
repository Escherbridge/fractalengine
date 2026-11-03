//! Whole-selection preview and checked commit; see node_manager/AGENTS.md.

use super::{
    gimbal_interaction::{axis_screen_dir, axis_vec, pick_axis_constrained},
    router::{ClickArbiter, ClickPriority},
    DragRestore, NodeManager,
};
use crate::{
    actions::UiManager,
    editor_batch::{affine_edits, center, math::member_transform, BatchOrigin},
    gimbal::{GimbalAxis, GimbalGizmoGroup},
    navigation_manager::NavigationManager,
    panels::toolbar::Tool,
    plugin::{LocalUserRole, ToolState},
    verse_manager::VerseManager,
};
use bevy::prelude::*;
use fe_runtime::{app::DbCommandSender, editor_batch::EditorAffine};

#[derive(Debug)]
pub(super) struct GroupDrag {
    origin: BatchOrigin,
    originals: Vec<(Entity, Transform)>,
    tool: Tool,
    axis: GimbalAxis,
    start_cursor: Vec2,
    screen_dir: Vec2,
    pivot: Vec3,
    affine: EditorAffine,
    moved: bool,
}

impl GroupDrag {
    pub(super) fn contains_node(&self, node_id: &str) -> bool {
        self.origin
            .capture
            .members
            .iter()
            .any(|member| member.node_id == node_id)
    }
}

impl NodeManager {
    pub(super) fn cancel_group_drag(&mut self) {
        if let Some(drag) = self.group_drag.take() {
            self.cancelled_restores
                .extend(drag.originals.into_iter().map(|(entity, transform)| {
                    (
                        entity,
                        DragRestore {
                            translation: transform.translation,
                            rotation: transform.rotation,
                            scale: transform.scale,
                        },
                    )
                }));
        }
    }
}

pub(crate) fn tool_capability(
    capture: &fe_runtime::editor_batch::EditorBatchCapture,
    tool: Tool,
    uniform_scale: bool,
) -> Result<(), String> {
    let pivot = center(capture)?;
    for member in &capture.members {
        let transform = member_transform(member, capture.world_scale)?;
        if let Some(path) = &member.stamp_path {
            if capture
                .members
                .iter()
                .any(|member| member.node_id == path.state.node_id)
            {
                return Err("A path and its stamp cannot be transformed together".into());
            }
            if tool == Tool::Move || !transform.translation.abs_diff_eq(pivot, 0.0001) {
                return Err("The selection includes a path-locked stamp; this center-based tool would move it off its path".into());
            }
        }
        if tool == Tool::Scale
            && !uniform_scale
            && member.stamp_path.is_none()
            && !member.state.properties.contains_key("gpx_points")
            && transform.rotation.dot(Quat::IDENTITY).abs() < 1.0 - 0.000001
        {
            return Err("Axis scaling of a rotated selected object would require an unsupported shear; uniform numeric scaling remains available".into());
        }
    }
    Ok(())
}

fn restore(originals: &[(Entity, Transform)], transforms: &mut Query<&mut Transform>) {
    for (entity, before) in originals {
        if let Ok(mut value) = transforms.get_mut(*entity) {
            *value = *before;
        }
    }
}

fn transformed(before: Transform, affine: &EditorAffine) -> Transform {
    let pivot = Vec3::from_array(affine.pivot.map(|value| value as f32));
    let rotation = Quat::from_array(affine.rotation);
    let scale = Vec3::from_array(affine.scale.map(|value| value as f32));
    Transform {
        translation: pivot
            + rotation * ((before.translation - pivot) * scale)
            + Vec3::from_array(affine.translation.map(|value| value as f32)),
        rotation: rotation * before.rotation,
        scale: before.scale * scale,
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PathPreviewContext<'w, 's> {
    shapes: Query<'w, 's, &'static super::TrackPickShape>,
    path: Res<'w, crate::gis::PathEditorState>,
    cache: Res<'w, crate::verse_manager::PathAssetCache>,
}

impl PathPreviewContext<'_, '_> {
    fn matches(
        &self,
        captured: &fe_runtime::editor_batch::EditorBatchMember,
        entity: Entity,
        petal: &str,
    ) -> bool {
        let stored_path = captured
            .stamp_path
            .as_ref()
            .map(|path| &path.state)
            .unwrap_or(&captured.state);
        let Some(points) = stored_path.properties.get("gpx_points") else {
            return true;
        };
        let Some(anchors) = fe_sdk::path_geometry::decode_anchor_rows(points) else {
            return false;
        };
        let expected = fe_sdk::path_geometry::flatten_anchor_path(
            &anchors,
            fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
        );
        if self.path.editing_track_id.as_deref() == Some(stored_path.node_id.as_str()) {
            let mut stored = crate::gis::decode_gpx_points(points);
            let mut draft = self.path.points.clone();
            for row in stored.iter_mut().chain(&mut draft) {
                row.time_seconds = Some(row.time_seconds.unwrap_or(0.0));
            }
            if stored != draft {
                return false;
            }
        }
        if captured.stamp_path.is_some() {
            return self
                .cache
                .stamp_selection_source(&stored_path.node_id, petal)
                .is_some_and(|(properties, _, rendered)| {
                    properties == &serde_json::Value::Object(stored_path.properties.clone())
                        && rendered == expected
                });
        }
        let Ok(shape) = self.shapes.get(entity) else {
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
                    .all(|(actual, expected)| {
                        actual.abs_diff_eq(Vec3::from_array(*expected), 0.0001)
                    }))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn interact(
    mut manager: ResMut<NodeManager>,
    mut ui: ResMut<UiManager>,
    undo: Res<crate::undo::UndoState>,
    nav: Res<NavigationManager>,
    role: Res<LocalUserRole>,
    tool: Res<ToolState>,
    mut arbiter: ResMut<ClickArbiter>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut transforms: Query<&mut Transform>,
    hierarchy: Res<VerseManager>,
    db: Res<DbCommandSender>,
    clock: Res<crate::notice::NoticeClock>,
    gizmos: Gizmos<GimbalGizmoGroup>,
    paths: PathPreviewContext,
) {
    let now = clock.elapsed_secs_f64();
    if let Some(mut drag) = manager.group_drag.take() {
        let current = drag.origin.matches(&nav, &manager)
            && role.can_edit()
            && tool.active_tool == drag.tool
            && drag.origin.capture.members.iter().all(|member| {
                hierarchy
                    .committed_versions
                    .get(&member.node_id)
                    .is_none_or(|version| member.history_version == Some(*version))
            })
            && drag
                .originals
                .iter()
                .all(|(entity, _)| transforms.contains(*entity));
        if !current
            || arbiter.is_fresh_press()
            || (!mouse.pressed(MouseButton::Left) && !mouse.just_released(MouseButton::Left))
        {
            restore(&drag.originals, &mut transforms);
            return;
        }
        if mouse.just_released(MouseButton::Left) {
            restore(&drag.originals, &mut transforms);
            if drag.moved {
                let edit = affine_edits(&drag.origin.capture, &drag.affine);
                let result = edit.and_then(|edit| {
                    ui.editor_batch
                        .start(drag.origin, edit, &nav, &manager, &db.0)
                });
                match result {
                    Ok(()) => ui.show_toast("Saving the complete selection…", now),
                    Err(error) => ui.show_advisory(error, now),
                }
            }
            return;
        }
        let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) else {
            restore(&drag.originals, &mut transforms);
            return;
        };
        let Ok((camera, camera_transform)) = cameras.single() else {
            restore(&drag.originals, &mut transforms);
            return;
        };
        let delta = cursor - drag.start_cursor;
        let movement = delta.dot(drag.screen_dir);
        drag.affine.translation = [0.0; 3];
        drag.affine.rotation = Quat::IDENTITY.to_array();
        drag.affine.scale = [1.0; 3];
        match drag.tool {
            Tool::Move => {
                let factor = (drag.pivot - camera_transform.translation())
                    .length()
                    .max(0.5)
                    * 0.002;
                let world_scale = f64::from(drag.origin.capture.world_scale);
                let position =
                    tool.snap
                        .move_position(drag.pivot, drag.axis, movement * factor, world_scale);
                drag.affine.translation = (position - drag.pivot).to_array().map(f64::from);
            }
            Tool::Rotate => {
                let tangent = Vec2::new(-drag.screen_dir.y, drag.screen_dir.x);
                drag.affine.rotation = Quat::from_axis_angle(
                    axis_vec(drag.axis),
                    tool.snap.rotation_angle(delta.dot(tangent) * 0.01),
                )
                .to_array();
            }
            Tool::Scale => {
                drag.affine.scale = tool
                    .snap
                    .scale_value(Vec3::ONE, drag.axis, 1.0 + movement * 0.005)
                    .to_array()
                    .map(f64::from);
            }
            _ => {
                restore(&drag.originals, &mut transforms);
                return;
            }
        }
        drag.moved = drag
            .affine
            .translation
            .iter()
            .any(|value| value.abs() > 0.00001)
            || drag
                .affine
                .scale
                .iter()
                .any(|value| (*value - 1.0).abs() > 0.00001)
            || Quat::from_array(drag.affine.rotation)
                .dot(Quat::IDENTITY)
                .abs()
                < 1.0 - 0.000001;
        if !tool.snap.axis.allows(drag.axis) {
            restore(&drag.originals, &mut transforms);
            return;
        }
        if drag.originals.iter().any(|(_, before)| {
            let value = transformed(*before, &drag.affine);
            !value.translation.is_finite()
                || !value.rotation.is_finite()
                || !value.scale.is_finite()
        }) {
            restore(&drag.originals, &mut transforms);
            ui.show_advisory("The complete preview was canceled because the transform exceeds the supported numeric range", now);
            return;
        }
        for (entity, before) in &drag.originals {
            if let Ok(mut value) = transforms.get_mut(*entity) {
                *value = transformed(*before, &drag.affine);
            }
        }
        crate::gimbal::draw_gimbal(
            drag.pivot + Vec3::from_array(drag.affine.translation.map(|value| value as f32)),
            drag.tool,
            Some(drag.axis),
            gizmos,
        );
        let _ = camera;
        manager.group_drag = Some(drag);
        return;
    }
    if !role.can_edit()
        || manager.selection_count() == 0
        || !matches!(tool.active_tool, Tool::Move | Tool::Rotate | Tool::Scale)
        || ui.editor_batch.is_pending()
        || undo.is_pending()
    {
        return;
    }
    let Some(capture) = ui.editor_batch.capture(&nav, &manager) else {
        return;
    };
    if manager.selection_count() == 1
        && (!crate::editor_batch::has_derived_transform(capture)
            || paths.path.selected_point.is_some()
            || paths.path.selected_segment.is_some())
    {
        return;
    }
    if tool_capability(capture, tool.active_tool, tool.snap.uniform_scale).is_err() {
        return;
    }
    let Ok(pivot) = center(capture) else {
        return;
    };
    let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) else {
        return;
    };
    let Ok((camera, camera_transform)) = cameras.single() else {
        return;
    };
    let axis = pick_axis_constrained(
        tool.active_tool,
        cursor,
        pivot,
        camera,
        camera_transform,
        tool.snap.axis,
    );
    crate::gimbal::draw_gimbal(pivot, tool.active_tool, axis, gizmos);
    if !role.can_edit() || !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    let Some(axis) = axis else {
        return;
    };
    let origin = match ui.editor_batch.origin(&manager) {
        Ok(origin) => origin,
        Err(error) => {
            ui.show_advisory(error, now);
            return;
        }
    };
    let mut originals = Vec::new();
    for (selected, captured) in manager.members().zip(&origin.capture.members) {
        let expected = member_transform(captured, origin.capture.world_scale);
        let Ok(actual) = transforms.get(selected.entity) else {
            ui.show_advisory(
                "A selected object is unavailable; no transform started",
                now,
            );
            return;
        };
        if !expected.is_ok_and(|expected| {
            crate::undo::transform_value(&expected).matches(&crate::undo::transform_value(actual))
        }) {
            ui.show_advisory("The rendered selection differs from its stored capture; refresh before transforming", now);
            return;
        }
        if !paths.matches(captured, selected.entity, &origin.capture.petal_id) {
            ui.show_advisory("A selected path or stamp has unsaved, changed or unavailable rendered geometry; the complete transform was refused", now);
            return;
        }
        originals.push((selected.entity, *actual));
    }
    if !arbiter.claim(ClickPriority::Gimbal) {
        return;
    }
    manager.group_drag = Some(GroupDrag {
        origin,
        originals,
        tool: tool.active_tool,
        axis,
        start_cursor: cursor,
        screen_dir: axis_screen_dir(axis, pivot, camera, camera_transform),
        pivot,
        affine: EditorAffine {
            pivot: pivot.to_array().map(f64::from),
            translation: [0.0; 3],
            rotation: Quat::IDENTITY.to_array(),
            scale: [1.0; 3],
        },
        moved: false,
    });
}
