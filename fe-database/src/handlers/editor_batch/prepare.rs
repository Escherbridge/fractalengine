//! Prepare all targets before one containing transaction has any effects.

use super::{capture, creation, geometry, groups as group_capture, stamp, PreparedBatch};
use crate::{
    handlers::{field_def, node_history, node_view, stamp_overrides, visibility_groups},
    repo::Db,
};
use bevy::math::{EulerRot, Quat};
use fe_runtime::editor_batch::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

struct Staged {
    state: Value,
    previous: Option<Value>,
    facts: Vec<(String, i64, Value)>,
}

impl Staged {
    fn change(&mut self, scope: &str, op: &str, data: Value, schemas: Value) -> anyhow::Result<()> {
        let change = node_history::prepare_change(
            self.state.clone(),
            self.previous.as_ref(),
            scope,
            op,
            data,
            schemas,
        )?;
        self.state = change.after;
        self.previous = change
            .facts
            .last()
            .map(|(_, version, payload)| json!({"row_version":version,"payload":payload}));
        self.facts.extend(change.facts);
        Ok(())
    }
}

pub(crate) async fn prepare(
    db: &Db,
    request: &EditorBatchRequest,
    request_id: &str,
    actor: &str,
) -> anyhow::Result<PreparedBatch> {
    super::validate_request_id(request_id)?;
    capture::bounded(&request.capture)?;
    let ids: Vec<_> = request
        .capture
        .members
        .iter()
        .map(|m| m.node_id.clone())
        .collect();
    let captured = group_capture::prepare_target(
        db,
        &request.capture.scope,
        &request.capture.petal_id,
        &ids,
        &request.capture.target,
        actor,
        fe_policy::RoleLevel::Editor,
        Some(request_id),
    )
    .await?;
    anyhow::ensure!(
        serde_json::to_value(&captured.view)? == serde_json::to_value(&request.capture)?,
        "selection, versions or source context changed before batch use"
    );
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
        ) if delete_request_id == edited => {}
        (EditorBatchTarget::Nodes, edit)
            if !matches!(
                edit,
                EditorBatchEdit::DeleteGroup { .. } | EditorBatchEdit::RestoreDeletedGroup { .. }
            ) => {}
        _ => anyhow::bail!("edit does not match the server-derived Group target"),
    }
    let scope = &captured.view.scope;
    let mut guards = captured.guards;
    let mut bindings = captured.bindings;
    let mut writes = Vec::new();
    let mut staged: Vec<_> = captured
        .nodes
        .iter()
        .map(|n| Staged {
            state: n.state.clone(),
            previous: n.previous.clone(),
            facts: vec![],
        })
        .collect();
    let mut groups = captured.view.groups.clone();
    let mut created_group_id = None;
    match &request.edit {
        EditorBatchEdit::Targets(edits) => {
            anyhow::ensure!(
                edits
                    .iter()
                    .map(|e| e.node_id.as_str())
                    .eq(ids.iter().map(String::as_str)),
                "edits must exactly match the complete ordered selection"
            );
            let transformed_paths: BTreeSet<_> = edits
                .iter()
                .filter(|edit| {
                    matches!(
                        edit.mutation,
                        EditorTargetMutation::PathAffine(_)
                            | EditorTargetMutation::PathRestore { .. }
                    )
                })
                .map(|edit| edit.node_id.as_str())
                .collect();
            for (index, edit) in edits.iter().enumerate() {
                let member = &captured.view.members[index];
                let node = &mut staged[index];
                let stamp = member.stamp_path.is_some();
                let path = member.state.properties.contains_key("gpx_points");
                let prefix = format!("batch_edit_{index}");
                let mut stamp_writes = false;
                match &edit.mutation {
                    EditorTargetMutation::Transform(transform) => {
                        anyhow::ensure!(
                            !stamp && !path,
                            "derived paths and stamps require their typed batch adapter"
                        );
                        anyhow::ensure!(
                            transform
                                .position
                                .iter()
                                .chain(&transform.scale)
                                .all(|v| v.is_finite() && (*v as f32).is_finite())
                                && transform.rotation.iter().all(|v| v.is_finite()),
                            "batch transform exceeds render precision"
                        );
                        let rotation = Quat::from_array(transform.rotation);
                        anyhow::ensure!(
                            (rotation.length_squared() - 1.0).abs() <= 0.0001,
                            "batch rotation must be a unit quaternion"
                        );
                        let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
                        let data = json!({"position":transform.position.map(|v|v as f32),"rotation":[x,y,z],"scale":transform.scale.map(|v|v as f32)});
                        let old =
                            fe_runtime::node_history::NodeTransformValue::from_row(&member.state)
                                .ok_or_else(|| anyhow::anyhow!("stored transform unavailable"))?;
                        if !old.matches(transform) {
                            node.change(scope, "full_transform", data, json!({}))?;
                        }
                    }
                    EditorTargetMutation::PathAffine(delta) => {
                        anyhow::ensure!(
                            path && !stamp,
                            "whole-path affine requires a stored route"
                        );
                        let points = delta
                            .transform_path(&node.state["properties"]["gpx_points"])
                            .map_err(anyhow::Error::msg)?;
                        property(
                            db,
                            node,
                            scope,
                            "gpx_points",
                            Some(&points),
                            &prefix,
                            &mut guards,
                            &mut bindings,
                        )
                        .await?;
                    }
                    EditorTargetMutation::PathRestore { points } => {
                        anyhow::ensure!(
                            path && !stamp,
                            "whole-path restore requires a stored route"
                        );
                        geometry::validate_restore(
                            &node.state["properties"]["gpx_points"],
                            points,
                        )?;
                        property(
                            db,
                            node,
                            scope,
                            "gpx_points",
                            Some(points),
                            &prefix,
                            &mut guards,
                            &mut bindings,
                        )
                        .await?;
                    }
                    EditorTargetMutation::StampOverrides(edit) => {
                        let source = member
                            .stamp_path
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("stamp batch target is not promoted"))?;
                        anyhow::ensure!(
                            !transformed_paths.contains(source.state.node_id.as_str()),
                            "selecting a stamp and its transformed owning path is a conflicting batch dependency"
                        );
                        stamp::validate(edit, member, scope, captured.view.world_scale)?;
                        let parts = stamp_overrides::prepare(db, edit, request_id, actor)
                            .await?
                            .into_batch_parts();
                        let (statements, mut values) =
                            stamp::namespace(parts.statements, parts.bindings, &prefix);
                        for statement in statements {
                            if statement.trim_start().starts_with("UPDATE ") {
                                writes.push(statement);
                            } else {
                                guards.push(statement);
                            }
                        }
                        bindings.append(&mut values);
                        node.state = parts
                            .facts
                            .last()
                            .ok_or_else(|| anyhow::anyhow!("stamp batch facts unavailable"))?
                            .2["after_state"]
                            .clone();
                        node.previous = Some(json!({"row_version":parts.receipt.history_version}));
                        node.facts = parts.facts;
                        stamp_writes = true;
                    }
                    EditorTargetMutation::StampRestore { edit, overrides } => {
                        let source = member.stamp_path.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("stamp restore target is not promoted")
                        })?;
                        anyhow::ensure!(
                            !transformed_paths.contains(source.state.node_id.as_str()),
                            "stamp restore conflicts with its transformed owning path"
                        );
                        stamp::validate(edit, member, scope, captured.view.world_scale)?;
                        stamp::validate_restore(
                            edit,
                            overrides,
                            member,
                            captured.view.world_scale,
                        )?;
                        let parts = stamp_overrides::prepare(db, edit, request_id, actor)
                            .await?
                            .into_batch_parts();
                        let (statements, mut values) =
                            stamp::namespace(parts.statements, parts.bindings, &prefix);
                        guards.extend(
                            statements
                                .into_iter()
                                .filter(|sql| !sql.trim_start().starts_with("UPDATE ")),
                        );
                        bindings.append(&mut values);
                        for (field, (key, representation)) in [
                            ("stamp.override.scale", &overrides.scale),
                            ("stamp.override.rotation", &overrides.rotation),
                            ("stamp.override.arc_m", &overrides.arc_m),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let value = stamp::value(representation);
                            property(
                                db,
                                node,
                                scope,
                                key,
                                value.as_ref(),
                                &format!("{prefix}_restore_{field}"),
                                &mut guards,
                                &mut bindings,
                            )
                            .await?;
                        }
                    }
                    EditorTargetMutation::Properties(changes) => {
                        anyhow::ensure!(
                            !changes.is_empty() && changes.len() <= 32,
                            "bulk property edits require 1..=32 fields per object"
                        );
                        let mut keys = BTreeSet::new();
                        for (field, change) in changes.iter().enumerate() {
                            let (key, value) = match change {
                                EditorPropertyEdit::Set { key, value } => (key, Some(value)),
                                EditorPropertyEdit::Delete { key } => (key, None),
                            };
                            anyhow::ensure!(
                                !key.is_empty()
                                    && key.len() <= 256
                                    && keys.insert(key)
                                    && !reserved_property(key),
                                "duplicate, invalid or derived bulk property key"
                            );
                            property(
                                db,
                                node,
                                scope,
                                key,
                                value,
                                &format!("{prefix}_{field}"),
                                &mut guards,
                                &mut bindings,
                            )
                            .await?;
                        }
                    }
                }
                if !stamp_writes && !node.facts.is_empty() {
                    write_node(node, &prefix, &mut writes, &mut bindings);
                }
            }
        }
        EditorBatchEdit::CreateGroup { .. } | EditorBatchEdit::RedoCreatedGroup { .. } => {
            let replay =
                if let EditorBatchEdit::RedoCreatedGroup { undo_request_id } = &request.edit {
                    Some(
                        creation::prove_redo(
                            db,
                            &captured.view,
                            undo_request_id,
                            actor,
                            request_id,
                            &mut guards,
                            &mut bindings,
                        )
                        .await?,
                    )
                } else {
                    None
                };
            let (id, name) = match (&request.edit, &replay) {
                (EditorBatchEdit::CreateGroup { id, name }, _) => (id, name.as_str()),
                (_, Some(replay)) => (&replay.group_id, ""),
                _ => unreachable!(),
            };
            anyhow::ensure!(
                ulid::Ulid::from_string(id).is_ok() && !groups.iter().any(|g| &g.id == id),
                "new group ID must be a fresh ULID"
            );
            if let Some(replay) = replay.as_ref() {
                visibility_groups::fence_unreferenced_group_ids(
                    db,
                    &captured.view.petal_id,
                    &captured.view.groups,
                    &[id.clone()],
                    "batch_redo_group",
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                groups = replay.groups.clone();
            } else {
                visibility_groups::fence_new_group_ids(
                    db,
                    &captured.view.petal_id,
                    &captured.view.groups,
                    &[id.clone()],
                    visibility_groups::NewGroupFence {
                        prefix: "batch_new_group",
                        checked_request_id: Some(request_id),
                    },
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                let order = groups.iter().map(|g| g.order).fold(-1.0_f64, f64::max) + 1.0;
                groups.push(fe_runtime::messages::VisibilityGroupDto {
                    id: id.clone(),
                    name: name.trim().into(),
                    visible: true,
                    order,
                });
            }
            visibility_groups::validate_groups(&groups).map_err(anyhow::Error::msg)?;
            for (index, node) in staged.iter_mut().enumerate() {
                membership(
                    db,
                    node,
                    scope,
                    id,
                    true,
                    &format!("batch_member_{index}"),
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                if !node.facts.is_empty() {
                    write_node(
                        node,
                        &format!("batch_member_{index}"),
                        &mut writes,
                        &mut bindings,
                    );
                }
            }
            writes.push("UPDATE petal SET visibility_groups=$batch_new_registry WHERE petal_id=$batch_petal".into());
            bindings.push(("batch_new_registry".into(), json!(groups)));
            created_group_id = Some(id.clone());
        }
        EditorBatchEdit::Membership { group_id, member } => {
            anyhow::ensure!(
                groups.iter().any(|g| &g.id == group_id),
                "membership group is not registered in this Petal"
            );
            for (index, node) in staged.iter_mut().enumerate() {
                let prefix = format!("batch_member_{index}");
                membership(
                    db,
                    node,
                    scope,
                    group_id,
                    *member,
                    &prefix,
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                if !node.facts.is_empty() {
                    write_node(node, &prefix, &mut writes, &mut bindings);
                }
            }
        }
        EditorBatchEdit::DeleteGroup { group_id }
        | EditorBatchEdit::UndoCreatedGroup { group_id, .. } => {
            if let EditorBatchEdit::UndoCreatedGroup {
                create_request_id,
                memberships,
                ..
            } = &request.edit
            {
                creation::prove_undo(
                    db,
                    &captured.view,
                    create_request_id,
                    group_id,
                    memberships,
                    actor,
                    request_id,
                    &mut guards,
                    &mut bindings,
                )
                .await?;
            }
            anyhow::ensure!(
                groups.iter().any(|g| &g.id == group_id),
                "deleted group is not registered"
            );
            let select = "SELECT VALUE node_id FROM node WHERE petal_id=$batch_petal AND properties['view.groups'] CONTAINS $batch_deleted_group AND node_id NOT IN $batch_target_ids LIMIT 1";
            let mut read = db
                .query(select)
                .bind(("batch_petal", captured.view.petal_id.clone()))
                .bind(("batch_deleted_group", group_id.clone()))
                .bind(("batch_target_ids", json!(ids)))
                .await?
                .check()?;
            let others: Vec<Value> = read.take(0)?;
            anyhow::ensure!(
                others.is_empty(),
                "group has members outside the captured selection; no group was deleted"
            );
            guards.push(format!(
                "IF array::len(({select})) != 0 {{ THROW 'group gained uncaptured members'; }}"
            ));
            bindings.extend([
                ("batch_deleted_group".into(), json!(group_id)),
                ("batch_target_ids".into(), json!(ids)),
            ]);
            groups.retain(|group| &group.id != group_id);
            if let EditorBatchEdit::UndoCreatedGroup { memberships, .. } = &request.edit {
                anyhow::ensure!(
                    memberships.len() == staged.len(),
                    "created Group inverse must cover every captured member"
                );
                for value in memberships {
                    if let EditorMembershipValue::Groups(ids) = value {
                        anyhow::ensure!(
                            !ids.contains(group_id),
                            "created Group inverse cannot retain its removed ID"
                        );
                    }
                }
            }
            for (index, node) in staged.iter_mut().enumerate() {
                let prefix = format!("batch_member_{index}");
                if let EditorBatchEdit::UndoCreatedGroup { memberships, .. } = &request.edit {
                    restore_membership(
                        db,
                        node,
                        scope,
                        &memberships[index],
                        &groups,
                        &prefix,
                        &mut guards,
                        &mut bindings,
                    )
                    .await?;
                } else {
                    membership(
                        db,
                        node,
                        scope,
                        group_id,
                        false,
                        &prefix,
                        &mut guards,
                        &mut bindings,
                    )
                    .await?;
                }
                if !node.facts.is_empty() {
                    write_node(node, &prefix, &mut writes, &mut bindings);
                }
            }
            writes.push("UPDATE petal SET visibility_groups=$batch_new_registry WHERE petal_id=$batch_petal".into());
            bindings.push(("batch_new_registry".into(), json!(groups)));
        }
        EditorBatchEdit::RestoreDeletedGroup { .. } => {
            let context = captured
                .view
                .deleted_group
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("durable Group deletion context is missing"))?;
            anyhow::ensure!(
                !groups.iter().any(|group| group.id == context.group_id),
                "restored Group ID collides with the current registry"
            );
            visibility_groups::fence_unreferenced_group_ids(
                db,
                &captured.view.petal_id,
                &groups,
                &[context.group_id.clone()],
                "batch_restore_group",
                &mut guards,
                &mut bindings,
            )
            .await?;
            anyhow::ensure!(
                context.memberships.len() == staged.len(),
                "Group restore must cover the exact proven target set"
            );
            groups = context.registry_before.clone();
            visibility_groups::validate_groups(&groups).map_err(anyhow::Error::msg)?;
            for (index, node) in staged.iter_mut().enumerate() {
                let prefix = format!("batch_member_{index}");
                restore_membership(
                    db,
                    node,
                    scope,
                    &context.memberships[index],
                    &groups,
                    &prefix,
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                if !node.facts.is_empty() {
                    write_node(node, &prefix, &mut writes, &mut bindings);
                }
            }
            writes.push("UPDATE petal SET visibility_groups=$batch_new_registry WHERE petal_id=$batch_petal".into());
            bindings.push(("batch_new_registry".into(), json!(groups)));
        }
        EditorBatchEdit::RestoreMemberships(previous) => {
            anyhow::ensure!(
                previous.len() == staged.len(),
                "membership inverse must cover the exact complete selection"
            );
            for (index, (node, representation)) in staged.iter_mut().zip(previous).enumerate() {
                let prefix = format!("batch_member_{index}");
                restore_membership(
                    db,
                    node,
                    scope,
                    representation,
                    &groups,
                    &prefix,
                    &mut guards,
                    &mut bindings,
                )
                .await?;
                if !node.facts.is_empty() {
                    write_node(node, &prefix, &mut writes, &mut bindings);
                }
            }
        }
    }
    let mut members = captured.view.members.clone();
    for (member, node) in members.iter_mut().zip(&staged) {
        member.state = serde_json::from_value(node.state.clone())?;
        member.history_version = node
            .previous
            .as_ref()
            .and_then(|row| row["row_version"].as_i64());
    }
    let sources: BTreeMap<_, _> = members
        .iter()
        .map(|member| {
            (
                member.node_id.clone(),
                (member.state.clone(), member.history_version),
            )
        })
        .collect();
    for member in &mut members {
        if let Some(source) = &mut member.stamp_path {
            if let Some((state, version)) = sources.get(&source.state.node_id) {
                source.state = state.clone();
                source.history_version = *version;
            }
        }
    }
    let receipt = EditorBatchReceipt {
        target: request.capture.target.clone(),
        request_id: request_id.into(),
        scope: scope.clone(),
        petal_id: captured.view.petal_id.clone(),
        members,
        groups,
        created_group_id,
    };
    let mut checks = Vec::new();
    for (index, (node, before)) in staged.iter().zip(&captured.nodes).enumerate() {
        let mut expected = if node.facts.is_empty() {
            before.raw.clone()
        } else {
            node.state.clone()
        };
        expected
            .as_object_mut()
            .expect("stored object")
            .remove("position");
        let prefix = format!("batch_confirm_{index}");
        bindings.extend([
            (format!("{prefix}_id"), json!(ids[index])),
            (format!("{prefix}_state"), json!([expected])),
            (format!("{prefix}_x"), node.state["position"][0].clone()),
            (format!("{prefix}_z"), node.state["position"][2].clone()),
        ]);
        checks.push(format!("IF (SELECT * OMIT id, position FROM node WHERE node_id=${prefix}_id) != ${prefix}_state {{ THROW 'batch materialization differs from receipt'; }}"));
        checks.push(format!("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=${prefix}_id AND position=<geometry<point>> [${prefix}_x,${prefix}_z])) != 1 {{ THROW 'batch materialized position differs from receipt'; }}"));
    }
    if receipt.groups != captured.view.groups {
        checks.push("IF (SELECT VALUE visibility_groups FROM petal WHERE petal_id=$batch_petal) != [$batch_new_registry] { THROW 'batch registry differs from receipt'; }".into());
    }
    let facts: Vec<_> = staged
        .into_iter()
        .zip(&ids)
        .flat_map(|(node, id)| {
            node.facts
                .into_iter()
                .map(move |(op, version, payload)| (id.clone(), op, version, payload))
        })
        .collect();
    capture::bounded(&(&request.capture, &receipt, &facts, &bindings))?;
    guards.push("IF array::len((SELECT VALUE id FROM op_log WHERE payload.editor_batch_request_id=$batch_request)) != 1 { THROW 'batch request already committed; reconcile its receipt'; }".into());
    bindings.push(("batch_request".into(), json!(request_id)));
    guards.extend(writes);
    Ok(PreparedBatch {
        guards,
        bindings,
        facts,
        checks,
        receipt,
        actor: actor.into(),
        delete_before: matches!(&request.edit, EditorBatchEdit::DeleteGroup { .. })
            .then(|| request.capture.clone()),
        create_before: matches!(
            &request.edit,
            EditorBatchEdit::CreateGroup { .. } | EditorBatchEdit::RedoCreatedGroup { .. }
        )
        .then(|| request.capture.clone()),
        undo_before: matches!(&request.edit, EditorBatchEdit::UndoCreatedGroup { .. })
            .then(|| request.capture.clone()),
        undo_of: match &request.edit {
            EditorBatchEdit::UndoCreatedGroup {
                create_request_id, ..
            } => Some(create_request_id.clone()),
            _ => None,
        },
        redo_of: match &request.edit {
            EditorBatchEdit::RedoCreatedGroup { undo_request_id } => Some(undo_request_id.clone()),
            _ => None,
        },
        restore_of: match &request.edit {
            EditorBatchEdit::RestoreDeletedGroup { delete_request_id } => {
                Some(delete_request_id.clone())
            }
            _ => None,
        },
    })
}

fn reserved_property(key: &str) -> bool {
    key.starts_with("stamp.override.")
        || matches!(
            key,
            "gpx_points"
                | "path_asset"
                | "node_kind"
                | "path_id"
                | "owning_path_id"
                | "instance_index"
                | "gpx_track_id"
                | "parent_id"
                | "view.groups"
                | "view.state"
        )
}

#[allow(clippy::too_many_arguments)]
async fn property(
    db: &Db,
    node: &mut Staged,
    scope: &str,
    key: &str,
    value: Option<&Value>,
    prefix: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<()> {
    let rules = field_def::capture_property_rules(db, scope, key).await?;
    let schemas = match value {
        Some(value) => field_def::validate_captured_property_rules(scope, key, value, &rules)?,
        None => json!({}),
    };
    guards.push(format!(
        "IF ({}) != ${prefix}_rules {{ THROW 'batch property schema changed'; }}",
        field_def::PROPERTY_RULES_SQL
            .replace("$key", &format!("${prefix}_key"))
            .replace("$scopes", &format!("${prefix}_scopes"))
    ));
    bindings.extend([
        (format!("{prefix}_key"), json!(key)),
        (
            format!("{prefix}_scopes"),
            json!(field_def::property_rule_scopes(scope)),
        ),
        (format!("{prefix}_rules"), json!(rules)),
    ]);
    if node.state["properties"].get(key) == value {
        return Ok(());
    }
    match value {
        Some(value) => node.change(
            scope,
            "property_set",
            json!({"key":key,"value":value}),
            schemas,
        ),
        None => node.change(scope, "property_deleted", json!({"key":key}), schemas),
    }
}

#[allow(clippy::too_many_arguments)]
async fn membership(
    db: &Db,
    node: &mut Staged,
    scope: &str,
    id: &str,
    add: bool,
    prefix: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<()> {
    let mut ids: Vec<String> = match node.state["properties"].get(node_view::VIEW_GROUPS_KEY) {
        None | Some(Value::Null) => vec![],
        Some(value) => serde_json::from_value(value.clone())?,
    };
    let exists = ids.iter().any(|existing| existing == id);
    if add && !exists {
        ids.push(id.into());
    } else if !add && exists {
        ids.retain(|existing| existing != id);
    } else {
        return Ok(());
    }
    node_view::validate_view_request(None, Some(&ids)).map_err(anyhow::Error::msg)?;
    let value = json!(ids);
    property(
        db,
        node,
        scope,
        node_view::VIEW_GROUPS_KEY,
        if ids.is_empty() { None } else { Some(&value) },
        prefix,
        guards,
        bindings,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn restore_membership(
    db: &Db,
    node: &mut Staged,
    scope: &str,
    representation: &EditorMembershipValue,
    groups: &[fe_runtime::messages::VisibilityGroupDto],
    prefix: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<()> {
    let old: Vec<String> = match node.state["properties"].get(node_view::VIEW_GROUPS_KEY) {
        None | Some(Value::Null) => vec![],
        Some(value) => serde_json::from_value(value.clone())?,
    };
    let value = match representation {
        EditorMembershipValue::Absent => None,
        EditorMembershipValue::Null => Some(Value::Null),
        EditorMembershipValue::Groups(ids) => {
            node_view::validate_view_request(None, Some(ids)).map_err(anyhow::Error::msg)?;
            anyhow::ensure!(
                ids.iter()
                    .all(|id| old.contains(id) || groups.iter().any(|group| &group.id == id)),
                "membership inverse references an unregistered group"
            );
            Some(json!(ids))
        }
    };
    property(
        db,
        node,
        scope,
        node_view::VIEW_GROUPS_KEY,
        value.as_ref(),
        prefix,
        guards,
        bindings,
    )
    .await
}

fn write_node(
    node: &Staged,
    prefix: &str,
    writes: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) {
    writes.push(format!("UPDATE node SET properties=${prefix}_properties, position=<geometry<point>> [${prefix}_x,${prefix}_z], elevation=${prefix}_y, rotation=${prefix}_rotation, scale=${prefix}_scale, edit_seq=${prefix}_seq WHERE node_id=${prefix}_id"));
    for (suffix, value) in [
        ("id", &node.state["node_id"]),
        ("properties", &node.state["properties"]),
        ("x", &node.state["position"][0]),
        ("y", &node.state["position"][1]),
        ("z", &node.state["position"][2]),
        ("rotation", &node.state["rotation"]),
        ("scale", &node.state["scale"]),
        ("seq", &node.state["edit_seq"]),
    ] {
        bindings.push((format!("{prefix}_{suffix}"), value.clone()));
    }
}
