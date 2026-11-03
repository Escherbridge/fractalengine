//! Complete capture, receipt and inverse validation; see editor_batch/AGENTS.md.

use super::{math, HistoryDirection, HistoryEntry};
use fe_runtime::{editor_batch::*, node_history::NodeTransformValue};
use serde_json::{Map, Value};

fn equal<T: serde::Serialize>(left: &T, right: &T) -> bool {
    serde_json::to_value(left)
        .ok()
        .zip(serde_json::to_value(right).ok())
        .is_some_and(|(left, right)| left == right)
}

pub(super) fn capture(
    value: &EditorBatchCapture,
    scope: &str,
    petal: &str,
    ids: &[String],
) -> Result<(), String> {
    if value.scope != scope
        || value.petal_id != petal
        || value.members.len() != ids.len()
        || (ids.is_empty() && matches!(&value.target, EditorBatchTarget::Nodes))
        || ids.len() > MAX_EDITOR_BATCH_TARGETS
        || fe_database::handlers::visibility_groups::validate_groups(&value.groups).is_err()
        || (matches!(
            &value.target,
            EditorBatchTarget::Nodes | EditorBatchTarget::Group { .. }
        ) && value.deleted_group.is_some())
        || value.deleted_group.as_ref().is_some_and(|context| {
            context.memberships.len() != value.members.len()
                || fe_database::handlers::visibility_groups::validate_groups(
                    &context.registry_before,
                )
                .is_err()
        })
        || value.petal_snapshot.is_empty()
        || !value.world_scale.is_finite()
        || value.world_scale <= 0.0
        || value
            .members
            .iter()
            .zip(ids)
            .enumerate()
            .any(|(index, (member, id))| {
                member.node_id != *id
                    || member.state.node_id != *id
                    || member.state.petal_id != petal
                    || member.state.tombstone.is_some()
                    || ids[..index].contains(id)
                    || member.history_version.is_some_and(|version| version <= 0)
                    || NodeTransformValue::from_row(&member.state)
                        .is_none_or(|value| !value.matches(&value))
            })
        || serde_json::to_vec(value).map_or(true, |value| value.len() > MAX_EDITOR_BATCH_BYTES)
    {
        return Err(
            "Selected-object read is incomplete, invalid or belongs to another scope".into(),
        );
    }
    Ok(())
}

pub(super) fn edit(request: &EditorBatchRequest) -> Result<(), String> {
    match (&request.capture.target, &request.edit) {
        (
            EditorBatchTarget::Group { group_id },
            EditorBatchEdit::DeleteGroup { group_id: edited },
        ) if group_id == edited => {}
        (
            EditorBatchTarget::DeletedGroup { delete_request_id },
            EditorBatchEdit::RestoreDeletedGroup {
                delete_request_id: edited,
            },
        ) if delete_request_id == edited && request.capture.deleted_group.is_some() => {}
        (EditorBatchTarget::Nodes, edit)
            if !matches!(
                edit,
                EditorBatchEdit::DeleteGroup { .. } | EditorBatchEdit::RestoreDeletedGroup { .. }
            ) && !request.capture.members.is_empty() => {}
        _ => return Err("Edit does not match the complete stored Group target".into()),
    }
    if let EditorBatchEdit::Targets(targets) = &request.edit {
        if targets.len() != request.capture.members.len()
            || targets
                .iter()
                .zip(&request.capture.members)
                .any(|(target, member)| target.node_id != member.node_id)
        {
            return Err(
                "The complete selected target set is required; no subset edit was sent".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn same_state(current: &EditorBatchCapture, expected: &EditorBatchCapture) -> bool {
    current.scope == expected.scope
        && current.petal_id == expected.petal_id
        && current.world_scale == expected.world_scale
        && equal(&current.members, &expected.members)
        && equal(&current.groups, &expected.groups)
}

fn memberships(properties: &Map<String, Value>) -> Vec<String> {
    properties
        .get("view.groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn write_memberships(properties: &mut Map<String, Value>, groups: Vec<String>) {
    if groups.is_empty() {
        properties.remove("view.groups");
    } else {
        properties.insert("view.groups".into(), serde_json::json!(groups));
    }
}

fn membership_value(properties: &Map<String, Value>) -> EditorMembershipValue {
    match properties.get("view.groups") {
        None => EditorMembershipValue::Absent,
        Some(Value::Null) => EditorMembershipValue::Null,
        Some(_) => EditorMembershipValue::Groups(memberships(properties)),
    }
}

fn restore_membership(properties: &mut Map<String, Value>, value: &EditorMembershipValue) {
    match value {
        EditorMembershipValue::Absent => {
            properties.remove("view.groups");
        }
        EditorMembershipValue::Null => {
            properties.insert("view.groups".into(), Value::Null);
        }
        EditorMembershipValue::Groups(groups) => {
            properties.insert("view.groups".into(), serde_json::json!(groups));
        }
    }
}

fn override_set(properties: &Map<String, Value>) -> EditorStampOverrideSet {
    let value = |key| match properties.get(key) {
        None => EditorStampOverrideValue::Absent,
        Some(Value::Null) => EditorStampOverrideValue::Null,
        Some(value) => EditorStampOverrideValue::Value(value.clone()),
    };
    EditorStampOverrideSet {
        scale: value("stamp.override.scale"),
        rotation: value("stamp.override.rotation"),
        arc_m: value("stamp.override.arc_m"),
    }
}

fn restore_stamp(properties: &mut Map<String, Value>, overrides: &EditorStampOverrideSet) {
    for (key, value) in [
        ("stamp.override.scale", &overrides.scale),
        ("stamp.override.rotation", &overrides.rotation),
        ("stamp.override.arc_m", &overrides.arc_m),
    ] {
        match value {
            EditorStampOverrideValue::Absent => {
                properties.remove(key);
            }
            EditorStampOverrideValue::Null => {
                properties.insert(key.into(), Value::Null);
            }
            EditorStampOverrideValue::Value(value) => {
                properties.insert(key.into(), value.clone());
            }
        }
    }
}

pub(super) fn receipt(
    receipt: &EditorBatchReceipt,
    request_id: &str,
    request: &EditorBatchRequest,
    replay: Option<&HistoryEntry>,
) -> Result<(), String> {
    let before = &request.capture;
    if serde_json::to_vec(receipt).map_or(true, |bytes| bytes.len() > MAX_EDITOR_BATCH_BYTES)
        || receipt.target != before.target
        || receipt.request_id != request_id
        || receipt.scope != before.scope
        || receipt.petal_id != before.petal_id
        || receipt.members.len() != before.members.len()
    {
        return Err(
            "Selection receipt is incomplete or unowned; reconciliation is required".into(),
        );
    }
    if !matches!(
        &request.edit,
        EditorBatchEdit::CreateGroup { .. } | EditorBatchEdit::RedoCreatedGroup { .. }
    ) && receipt.created_group_id.is_some()
    {
        return Err("Unexpected created group identity in selection receipt".into());
    }
    let mut groups = before.groups.clone();
    match &request.edit {
        EditorBatchEdit::CreateGroup { id, name } => {
            if receipt.created_group_id.as_deref() != Some(id)
                || groups.iter().any(|group| group.id == *id)
            {
                return Err("Created group identity differs from the submitted edit".into());
            }
            groups.push(fe_runtime::messages::VisibilityGroupDto {
                id: id.clone(),
                name: name.trim().into(),
                visible: true,
                order: groups
                    .iter()
                    .map(|group| group.order)
                    .fold(-1.0_f64, f64::max)
                    + 1.0,
            });
        }
        EditorBatchEdit::RedoCreatedGroup { undo_request_id } => {
            let entry = replay.ok_or("No owned creation history matches this Redo")?;
            let EditorBatchEdit::CreateGroup { id, .. } = &entry.forward else {
                return Err("Redo proof does not belong to a Group creation".into());
            };
            if entry.undo_create_request_id.as_ref() != Some(undo_request_id)
                || receipt.created_group_id.as_deref() != Some(id.as_str())
            {
                return Err("Creation Redo receipt differs from its owned proof".into());
            }
            groups = entry.after.groups.clone();
        }
        EditorBatchEdit::RestoreDeletedGroup { .. } => {
            groups = before
                .deleted_group
                .as_ref()
                .ok_or("Exact Group restore context is missing")?
                .registry_before
                .clone();
        }
        EditorBatchEdit::DeleteGroup { group_id }
        | EditorBatchEdit::UndoCreatedGroup { group_id, .. } => {
            groups.retain(|group| group.id != *group_id);
        }
        _ => {
            if receipt.created_group_id.is_some() {
                return Err("Unexpected group creation in selection receipt".into());
            }
        }
    }
    if !equal(&groups, &receipt.groups) {
        return Err("Selection receipt has an unexpected group registry".into());
    }
    for (index, (old, saved)) in before.members.iter().zip(&receipt.members).enumerate() {
        let mut expected_path = old.stamp_path.clone();
        if let Some(path) = &mut expected_path {
            if let Some(owner) = receipt
                .members
                .iter()
                .find(|member| member.node_id == path.state.node_id)
            {
                path.state = owner.state.clone();
                path.history_version = owner.history_version;
            }
        }
        if old.node_id != saved.node_id
            || saved.state.node_id != old.node_id
            || saved.state.petal_id != before.petal_id
            || !equal(&expected_path, &saved.stamp_path)
            || saved.history_version.is_none()
            || saved.history_version < old.history_version
        {
            return Err("Selection receipt changed a target, path context or version".into());
        }
        let mut expected = old.state.clone();
        match &request.edit {
            EditorBatchEdit::CreateGroup { id, .. } => {
                let mut members = memberships(&expected.properties);
                if !members.contains(id) {
                    members.push(id.clone());
                }
                write_memberships(&mut expected.properties, members);
            }
            EditorBatchEdit::RedoCreatedGroup { .. } => {
                let entry = replay.ok_or("No owned creation context matches Redo")?;
                let source = entry
                    .after
                    .members
                    .get(index)
                    .filter(|member| member.node_id == old.node_id)
                    .ok_or("Incomplete creation Redo context")?;
                match source.state.properties.get("view.groups") {
                    Some(value) => {
                        expected
                            .properties
                            .insert("view.groups".into(), value.clone());
                    }
                    None => {
                        expected.properties.remove("view.groups");
                    }
                }
            }
            EditorBatchEdit::DeleteGroup { group_id } => {
                let mut members = memberships(&expected.properties);
                members.retain(|id| id != group_id);
                write_memberships(&mut expected.properties, members);
            }
            EditorBatchEdit::Membership { group_id, member } => {
                let mut members = memberships(&expected.properties);
                if *member && !members.contains(group_id) {
                    members.push(group_id.clone());
                }
                if !member {
                    members.retain(|id| id != group_id);
                }
                write_memberships(&mut expected.properties, members);
            }
            EditorBatchEdit::RestoreDeletedGroup { .. } => {
                let context = before
                    .deleted_group
                    .as_ref()
                    .ok_or("Exact Group restore context is missing")?;
                restore_membership(
                    &mut expected.properties,
                    context
                        .memberships
                        .get(index)
                        .ok_or("Incomplete Group restore context")?,
                );
            }
            EditorBatchEdit::RestoreMemberships(members)
            | EditorBatchEdit::UndoCreatedGroup {
                memberships: members,
                ..
            } => {
                restore_membership(
                    &mut expected.properties,
                    members.get(index).ok_or("Incomplete membership inverse")?,
                );
            }
            EditorBatchEdit::Targets(targets) => match &targets
                .get(index)
                .ok_or("Incomplete target receipt")?
                .mutation
            {
                EditorTargetMutation::Transform(value) => {
                    let actual = NodeTransformValue::from_row(&saved.state)
                        .ok_or("Invalid saved transform")?;
                    if !value.matches(&actual) {
                        return Err(
                            "A saved transform differs from the complete submitted edit".into()
                        );
                    }
                    let changed = NodeTransformValue::from_row(&old.state)
                        .is_none_or(|previous| !value.matches(&previous));
                    expected.position = saved.state.position;
                    expected.elevation = saved.state.position[1];
                    expected.scale = saved.state.scale;
                    expected.rotation = saved.state.rotation.clone();
                    expected.edit_seq += i64::from(changed);
                }
                EditorTargetMutation::PathAffine(affine) => {
                    let value = math::path_points(
                        expected
                            .properties
                            .get("gpx_points")
                            .ok_or("Path points are unavailable")?,
                        affine,
                    )?;
                    expected.properties.insert("gpx_points".into(), value);
                }
                EditorTargetMutation::PathRestore { points } => {
                    expected
                        .properties
                        .insert("gpx_points".into(), points.clone());
                }
                EditorTargetMutation::StampOverrides(edit) => {
                    expected
                        .properties
                        .insert("stamp.override.scale".into(), serde_json::json!(edit.scale));
                    expected.properties.insert(
                        "stamp.override.rotation".into(),
                        serde_json::json!(edit.rotation),
                    );
                    expected
                        .properties
                        .insert("stamp.override.arc_m".into(), serde_json::json!(edit.arc_m));
                }
                EditorTargetMutation::StampRestore { overrides, .. } => {
                    restore_stamp(&mut expected.properties, overrides);
                }
                EditorTargetMutation::Properties(edits) => {
                    for edit in edits {
                        match edit {
                            EditorPropertyEdit::Set { key, value } => {
                                expected.properties.insert(key.clone(), value.clone());
                            }
                            EditorPropertyEdit::Delete { key } => {
                                expected.properties.remove(key);
                            }
                        }
                    }
                }
            },
        }
        if !equal(&expected, &saved.state)
            || (!equal(&expected, &old.state) && saved.history_version <= old.history_version)
        {
            return Err("A member of the selection receipt differs from its expected state; none of the receipt was projected".into());
        }
    }
    Ok(())
}

pub(super) fn replay_edit(
    entry: &HistoryEntry,
    current: &EditorBatchCapture,
    direction: HistoryDirection,
) -> Result<EditorBatchEdit, String> {
    let undo = direction == HistoryDirection::Undo;
    match &entry.forward {
        EditorBatchEdit::CreateGroup { id, .. } => {
            return Ok(if undo {
                EditorBatchEdit::UndoCreatedGroup {
                    create_request_id: entry
                        .create_request_id
                        .clone()
                        .ok_or("Exact Group creation proof is unavailable; no inverse was sent")?,
                    group_id: id.clone(),
                    memberships: entry
                        .before
                        .members
                        .iter()
                        .map(|member| membership_value(&member.state.properties))
                        .collect(),
                }
            } else {
                EditorBatchEdit::RedoCreatedGroup {
                    undo_request_id: entry
                        .undo_create_request_id
                        .clone()
                        .ok_or("Exact creation Undo proof is unavailable; no Redo was sent")?,
                }
            })
        }
        EditorBatchEdit::DeleteGroup { group_id } => {
            return Ok(if undo {
                EditorBatchEdit::RestoreDeletedGroup {
                    delete_request_id: entry
                        .delete_request_id
                        .clone()
                        .ok_or("Exact Group deletion proof is unavailable")?,
                }
            } else {
                EditorBatchEdit::DeleteGroup {
                    group_id: group_id.clone(),
                }
            });
        }
        EditorBatchEdit::Membership { group_id, member } if !undo => {
            return Ok(EditorBatchEdit::Membership {
                group_id: group_id.clone(),
                member: *member,
            })
        }
        EditorBatchEdit::Membership { .. } => {
            return Ok(EditorBatchEdit::RestoreMemberships(
                entry
                    .before
                    .members
                    .iter()
                    .map(|member| match member.state.properties.get("view.groups") {
                        None => EditorMembershipValue::Absent,
                        Some(Value::Null) => EditorMembershipValue::Null,
                        Some(_) => {
                            EditorMembershipValue::Groups(memberships(&member.state.properties))
                        }
                    })
                    .collect(),
            ))
        }
        EditorBatchEdit::Targets(_) => {}
        _ => return Err("This group operation has no supported checked inverse".into()),
    }
    let target = if undo { &entry.before } else { &entry.after };
    let EditorBatchEdit::Targets(forward) = &entry.forward else {
        unreachable!()
    };
    let mut edits = Vec::new();
    for ((before, now), original) in target.members.iter().zip(&current.members).zip(forward) {
        let mutation = match &original.mutation {
            EditorTargetMutation::Transform(_) => EditorTargetMutation::Transform(
                NodeTransformValue::from_row(&before.state)
                    .ok_or("Inverse transform is unavailable")?,
            ),
            EditorTargetMutation::PathAffine(_) | EditorTargetMutation::PathRestore { .. } => {
                EditorTargetMutation::PathRestore {
                    points: before
                        .state
                        .properties
                        .get("gpx_points")
                        .cloned()
                        .ok_or("Inverse path is unavailable")?,
                }
            }
            EditorTargetMutation::StampOverrides(_) | EditorTargetMutation::StampRestore { .. } => {
                let mut edit =
                    math::stamp_edit(now, &before.state.properties, current.world_scale)?;
                edit.scope = current.scope.clone();
                EditorTargetMutation::StampRestore {
                    edit,
                    overrides: override_set(&before.state.properties),
                }
            }
            EditorTargetMutation::Properties(properties) => EditorTargetMutation::Properties(
                properties
                    .iter()
                    .map(|property| {
                        let key = match property {
                            EditorPropertyEdit::Set { key, .. }
                            | EditorPropertyEdit::Delete { key } => key,
                        };
                        match before.state.properties.get(key) {
                            Some(value) => EditorPropertyEdit::Set {
                                key: key.clone(),
                                value: value.clone(),
                            },
                            None => EditorPropertyEdit::Delete { key: key.clone() },
                        }
                    })
                    .collect(),
            ),
        };
        edits.push(EditorTargetEdit {
            node_id: now.node_id.clone(),
            mutation,
        });
    }
    Ok(EditorBatchEdit::Targets(edits))
}
