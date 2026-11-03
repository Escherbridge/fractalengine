//! Typed selection transform adapters; see editor_batch/AGENTS.md.

use bevy::prelude::*;
use fe_runtime::{editor_batch::*, messages::StampOverrideEdit, node_history::NodeTransformValue};
use serde_json::Value;

pub(super) fn path_points(value: &Value, affine: &EditorAffine) -> Result<Value, String> {
    affine.transform_path(value)
}

pub(crate) fn member_transform(
    member: &EditorBatchMember,
    world_scale: f32,
) -> Result<Transform, String> {
    if member.stamp_path.is_some() {
        let edit = stamp_edit(member, &member.state.properties, world_scale)?;
        let flattened = fe_sdk::path_geometry::flatten_anchor_path(
            &edit.path_anchors,
            fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
        );
        let (distances, total) = fe_sdk::path_geometry::cumulative_distances(&flattened);
        let progress = if total > 0.0 {
            edit.arc_m * world_scale / total
        } else {
            0.0
        };
        let position = fe_sdk::path_geometry::position_at_progress(
            &flattened,
            &distances,
            total,
            progress.clamp(0.0, 1.0),
        );
        return Ok(Transform {
            translation: Vec3::from_array(position),
            rotation: Quat::from_array(edit.rotation),
            scale: Vec3::from_array(edit.scale),
        });
    }
    if let Some(points) = member.state.properties.get("gpx_points") {
        let anchors =
            fe_sdk::path_geometry::decode_anchor_rows(points).ok_or("Path geometry is invalid")?;
        let points = fe_sdk::path_geometry::flatten_anchor_path(
            &anchors,
            fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
        );
        if points.is_empty() {
            return Err("Empty paths cannot participate in a selection transform".into());
        }
        let center = (points
            .iter()
            .map(|point| bevy::math::DVec3::from_array(point.map(f64::from)))
            .sum::<bevy::math::DVec3>()
            / points.len() as f64)
            .as_vec3();
        if !center.is_finite() {
            return Err("Path center exceeds supported render precision".into());
        }
        return Ok(Transform::from_translation(center));
    }
    let value = NodeTransformValue::from_row(&member.state)
        .ok_or("An object's transform is unavailable")?;
    Ok(Transform {
        translation: Vec3::from_array(value.position.map(|value| value as f32)),
        rotation: Quat::from_array(value.rotation),
        scale: Vec3::from_array(value.scale.map(|value| value as f32)),
    })
}

pub(super) fn stamp_edit(
    member: &EditorBatchMember,
    properties: &serde_json::Map<String, Value>,
    world_scale: f32,
) -> Result<StampOverrideEdit, String> {
    use crate::actions::asset::{STAMP_ARC_KEY, STAMP_ROTATION_KEY, STAMP_SCALE_KEY};
    let path = member
        .stamp_path
        .as_ref()
        .ok_or("The stamp's stored path context is unavailable")?;
    let path_id = properties
        .get("owning_path_id")
        .or_else(|| properties.get("path_id"))
        .and_then(Value::as_str)
        .ok_or("Stamp path identity is unavailable")?;
    if path_id != path.state.node_id {
        return Err("Stamp and captured path disagree".into());
    }
    let index = properties
        .get("instance_index")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or("Stamp instance identity is unavailable")?;
    let descriptor = path
        .state
        .properties
        .get("path_asset")
        .ok_or("Stamp path descriptor is unavailable")?;
    let parsed = fe_sdk::path_asset::PathAssetDescriptor::from_json(descriptor)
        .map_err(|_| "Stamp path descriptor is invalid")?;
    let anchors = path
        .state
        .properties
        .get("gpx_points")
        .and_then(fe_sdk::path_geometry::decode_anchor_rows)
        .ok_or("Stamp path anchors are unavailable")?;
    let points = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    let (default_rotation, default_arc) =
        crate::verse_manager::stamp_editor_defaults(&points, &parsed, world_scale, index)
            .ok_or("Stamp is no longer part of the path")?;
    let scale = properties
        .get(STAMP_SCALE_KEY)
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<[f32; 3]>)
        .transpose()
        .map_err(|_| "Invalid stamp scale")?
        .unwrap_or([1.0; 3]);
    let rotation = properties
        .get(STAMP_ROTATION_KEY)
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<[f32; 4]>)
        .transpose()
        .map_err(|_| "Invalid stamp rotation")?
        .unwrap_or(default_rotation);
    let arc_m = match properties.get(STAMP_ARC_KEY) {
        None | Some(Value::Null) => default_arc,
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or("Invalid stamp arc override")? as f32,
    };
    Ok(StampOverrideEdit {
        node_id: member.node_id.clone(),
        path_id: path_id.into(),
        instance_index: index,
        scope: String::new(),
        history_version: member.history_version,
        path_version: path.history_version,
        before_properties: Value::Object(member.state.properties.clone()),
        path_anchors: anchors,
        path_descriptor: descriptor.clone(),
        scale,
        rotation,
        arc_m,
    })
}

pub(crate) fn center(capture: &EditorBatchCapture) -> Result<Vec3, String> {
    if capture.members.is_empty() {
        return Err("Select at least one object".into());
    }
    let mut center = bevy::math::DVec3::ZERO;
    for member in &capture.members {
        center += member_transform(member, capture.world_scale)?
            .translation
            .as_dvec3();
    }
    let center = (center / capture.members.len() as f64).as_vec3();
    if !center.is_finite() {
        return Err("Selection center is outside the supported numeric range".into());
    }
    Ok(center)
}

pub(crate) fn affine_edits(
    capture: &EditorBatchCapture,
    affine: &EditorAffine,
) -> Result<EditorBatchEdit, String> {
    let pivot = Vec3::from_array(affine.pivot.map(|value| value as f32));
    let translation = Vec3::from_array(affine.translation.map(|value| value as f32));
    let rotation = Quat::from_array(affine.rotation);
    let scale = Vec3::from_array(affine.scale.map(|value| value as f32));
    if !pivot.is_finite()
        || !translation.is_finite()
        || !rotation.is_finite()
        || !scale.is_finite()
        || rotation.length_squared() <= f32::EPSILON
        || scale.min_element() <= 0.0
    {
        return Err("Selection transform must use finite values and positive scale factors".into());
    }
    let rotation = rotation.normalize();
    let mut targets = Vec::new();
    for member in &capture.members {
        let before = member_transform(member, capture.world_scale)?;
        if member.stamp_path.is_none()
            && !member.state.properties.contains_key("gpx_points")
            && (scale.x - scale.y).abs().max((scale.y - scale.z).abs()) > 0.0001
            && before.rotation.dot(Quat::IDENTITY).abs() < 1.0 - 0.000001
        {
            return Err("The complete edit was refused: non-uniform scaling of a rotated object would require an unsupported shear".into());
        }
        let after = Transform {
            translation: pivot + rotation * ((before.translation - pivot) * scale) + translation,
            rotation: rotation * before.rotation,
            scale: before.scale * scale,
        };
        let mutation = if member.stamp_path.is_some() {
            if capture.members.iter().any(|selected| {
                member
                    .stamp_path
                    .as_ref()
                    .is_some_and(|path| selected.node_id == path.state.node_id)
            }) {
                return Err(
                    "A path and its stamp cannot be transformed in the same selection".into(),
                );
            }
            if !after.translation.abs_diff_eq(before.translation, 0.0001) {
                return Err("The complete edit was refused: a selected stamp is path-locked and cannot move around the selection center".into());
            }
            let mut edit = stamp_edit(member, &member.state.properties, capture.world_scale)?;
            edit.scope = capture.scope.clone();
            edit.scale = after.scale.to_array();
            edit.rotation = after.rotation.normalize().to_array();
            EditorTargetMutation::StampOverrides(edit)
        } else if member.state.properties.contains_key("gpx_points") {
            path_points(&member.state.properties["gpx_points"], affine)?;
            EditorTargetMutation::PathAffine(affine.clone())
        } else {
            EditorTargetMutation::Transform(crate::undo::transform_value(&after))
        };
        targets.push(EditorTargetEdit {
            node_id: member.node_id.clone(),
            mutation,
        });
    }
    Ok(EditorBatchEdit::Targets(targets))
}
