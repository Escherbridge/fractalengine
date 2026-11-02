//! Checked local editor mutations; see handlers/AGENTS.md section editor-undo.

use super::{entity, field_def, node_history};
use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::{
    messages::NodeMutation,
    node_history::{
        NodeEditBefore, NodeEditPrecondition, NodeMutationReceipt, NodeRowSnapshot,
        NodeTransformValue, PersistStatus,
    },
};
use serde_json::{json, Value};

pub(crate) async fn commit(
    db: &Db,
    node_id: &str,
    mutation: &NodeMutation,
    expected: &NodeEditPrecondition,
    request_id: &str,
    local_did: &str,
) -> anyhow::Result<NodeMutationReceipt> {
    anyhow::ensure!(
        !request_id.is_empty() && request_id.len() <= 128,
        "invalid editor request identity"
    );
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("object not found or not permitted"))?;
    anyhow::ensure!(
        scope == expected.scope,
        "Object moved since this edit; Undo skipped"
    );
    let permission =
        entity::prepare_scope_permission(db, &scope, local_did, fe_policy::RoleLevel::Editor)
            .await?;
    let (mut sql, mut bindings) = permission.into_parts();
    let mut response = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$edit_node")
        .bind(("edit_node", node_id.to_string()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() == 1 && rows[0].get("tombstone").is_none_or(Value::is_null),
        "object not found or not permitted"
    );
    let mut raw = rows[0].clone();
    let before = node_history::normalize_snapshot(raw.clone())?;
    let typed: NodeRowSnapshot = serde_json::from_value(before.clone())?;
    let parts = crate::parse_scope(&scope)?;
    anyhow::ensure!(
        parts.petal_id.as_deref() == Some(typed.petal_id.as_str()),
        "object ancestry changed"
    );
    let previous = node_history::latest(db, node_id).await?;
    let version = previous
        .as_ref()
        .and_then(|row| row["row_version"].as_i64());
    anyhow::ensure!(
        expected.history_version.is_none() || expected.history_version == version,
        "Object changed since this edit; Undo skipped"
    );
    let (operation, data, kind, schemas) = match (&expected.before, mutation) {
        (
            NodeEditBefore::Transform(wanted),
            NodeMutation::FullTransform {
                position,
                rotation,
                scale,
            },
        ) => {
            let current = NodeTransformValue::from_row(&typed)
                .ok_or_else(|| anyhow::anyhow!("invalid stored transform"))?;
            anyhow::ensure!(
                wanted.matches(&current),
                "Object changed since this edit; Undo skipped"
            );
            anyhow::ensure!(
                position
                    .iter()
                    .chain(rotation)
                    .chain(scale)
                    .all(|v| v.is_finite()),
                "transform components must be finite"
            );
            if typed.properties.get("node_kind").and_then(Value::as_str) == Some("stamp") {
                anyhow::ensure!(
                    current
                        .position
                        .iter()
                        .zip(position)
                        .all(|(a, b)| (*a - f64::from(*b)).abs() <= 0.0001),
                    "stamp position is path-derived"
                );
            }
            (
                "full_transform",
                json!({"position":position,"rotation":rotation,"scale":scale}),
                OpType::TransformUpdate,
                json!({}),
            )
        }
        (NodeEditBefore::PathPoints(wanted), NodeMutation::PropertySet { key, value })
            if key == "gpx_points" =>
        {
            anyhow::ensure!(
                typed.properties.get(key) == Some(wanted),
                "Path changed since this edit; Undo skipped"
            );
            validate_point_move(wanted, value)?;
            let rules = field_def::capture_property_rules(db, &scope, key).await?;
            let schemas = field_def::validate_captured_property_rules(&scope, key, value, &rules)?;
            sql.push(format!(
                "IF ({}) != $edit_rules {{ THROW 'path schema changed'; }}",
                field_def::PROPERTY_RULES_SQL
                    .replace("$key", "$edit_key")
                    .replace("$scopes", "$edit_scopes")
            ));
            bindings.extend([
                ("edit_rules".into(), json!(rules)),
                ("edit_key".into(), json!(key)),
                (
                    "edit_scopes".into(),
                    json!(field_def::property_rule_scopes(&scope)),
                ),
            ]);
            (
                "property_set",
                json!({"key":key,"value":value}),
                OpType::PropertySet,
                schemas,
            )
        }
        _ => anyhow::bail!("Undo supports full transforms and single path-point moves only"),
    };
    let prepared = node_history::prepare_change(
        before,
        previous.as_ref(),
        &scope,
        operation,
        data.clone(),
        schemas,
    )?;
    anyhow::ensure!(prepared.version > 0, "invalid stored edit history version");
    let receipt = NodeMutationReceipt {
        node_id: node_id.into(),
        history_version: prepared.version,
        state: serde_json::from_value(prepared.after.clone())?,
        persist: PersistStatus::Committed,
    };
    raw.as_object_mut().unwrap().remove("position");
    let (scope_sql, scope_bindings) = node_history::scope_fence(&scope, Some(node_id))?;
    sql.extend(scope_sql);
    bindings.extend(scope_bindings);
    sql.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$edit_node) != $edit_before { THROW 'Object changed since this edit; Undo skipped'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$edit_node AND position=<geometry<point>> [$edit_x,$edit_z])) != 1 { THROW 'Object changed since this edit; Undo skipped'; }".into(),
        "IF (SELECT * OMIT id FROM node_log WHERE node_id=$edit_node ORDER BY row_version DESC LIMIT 1) != $edit_history { THROW 'Object changed since this edit; Undo skipped'; }".into(),
        "IF array::len((SELECT VALUE log_id FROM node_log WHERE node_id=$edit_node AND payload.editor_request_id=$edit_request)) != 0 { THROW 'editor request already committed; reconcile its receipt'; }".into(),
    ]);
    bindings.extend([
        ("edit_node".into(), json!(node_id)),
        ("edit_before".into(), json!([raw])),
        ("edit_x".into(), json!(typed.position[0])),
        ("edit_z".into(), json!(typed.position[2])),
        (
            "edit_history".into(),
            json!(previous.into_iter().collect::<Vec<_>>()),
        ),
        ("edit_request".into(), json!(request_id)),
    ]);
    match mutation {
        NodeMutation::FullTransform {
            position,
            rotation,
            scale,
        } => {
            sql.push("UPDATE node SET position=<geometry<point>> [$edit_new_x,$edit_new_z], elevation=$edit_new_y, rotation=$edit_rotation, scale=$edit_scale, edit_seq=edit_seq+1 WHERE node_id=$edit_node".into());
            bindings.extend([
                ("edit_new_x".into(), json!(position[0] as f64)),
                ("edit_new_y".into(), json!(position[1] as f64)),
                ("edit_new_z".into(), json!(position[2] as f64)),
                ("edit_rotation".into(), json!(rotation)),
                ("edit_scale".into(), json!(scale)),
            ]);
        }
        NodeMutation::PropertySet { value, .. } => {
            sql.push(
                "UPDATE node SET properties.gpx_points=$edit_points WHERE node_id=$edit_node"
                    .into(),
            );
            bindings.push(("edit_points".into(), value.clone()));
        }
        _ => unreachable!(),
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.into()),
        op_type: kind,
        payload: json!({"node_id":node_id,"data":data,"editor_request_id":request_id}),
        sig: "00".repeat(64),
    };
    crate::op_log::commit_operation_atomic_with(db,entry,|hlc|{
        for (index,(op,version,mut payload)) in prepared.facts.into_iter().enumerate() {
            if op!="baseline" {payload["editor_request_id"]=json!(request_id);}
            let name=format!("edit_fact_{index}");sql.push(format!("CREATE node_log CONTENT ${name}"));
            bindings.push((name,json!({"log_id":ulid::Ulid::new().to_string(),"node_id":node_id,"hlc_timestamp":hlc as i64,"op":op,"source_did":local_did,"payload":payload,"row_version":version,"created_at":chrono::Utc::now().to_rfc3339()})));
        }
        (sql,bindings)
    }).await?;
    Ok(receipt)
}

fn validate_point_move(before: &Value, after: &Value) -> anyhow::Result<()> {
    fe_sdk::property_schema::validate_persistable_numbers(after).map_err(anyhow::Error::msg)?;
    let old = before
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("path points must be an array"))?;
    let new = after
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("path points must be an array"))?;
    anyhow::ensure!(old.len() == new.len(), "path point identity changed");
    let mut changed = 0;
    for (a, b) in old.iter().zip(new) {
        let a = a
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid path point"))?;
        let b = b
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid path point"))?;
        anyhow::ensure!(
            a.len() >= 4 && a.len() == b.len() && a[3..] == b[3..],
            "path point metadata changed"
        );
        anyhow::ensure!(
            b[..3]
                .iter()
                .all(|v| v.as_f64().is_some_and(f64::is_finite)),
            "path point coordinates must be finite"
        );
        changed += usize::from(a[..3] != b[..3]);
    }
    anyhow::ensure!(changed == 1, "Undo covers one changed path point");
    Ok(())
}

pub(crate) async fn resolve(
    db: &Db,
    node_id: &str,
    petal_id: &str,
    request_id: &str,
    local_did: &str,
) -> anyhow::Result<(Option<NodeMutationReceipt>, NodeRowSnapshot)> {
    anyhow::ensure!(
        !request_id.is_empty() && request_id.len() <= 128,
        "invalid editor request identity"
    );
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("object not found or not permitted"))?;
    anyhow::ensure!(
        crate::parse_scope(&scope)?.petal_id.as_deref() == Some(petal_id),
        "object moved since this request"
    );
    let permission =
        entity::prepare_scope_permission(db, &scope, local_did, fe_policy::RoleLevel::Viewer)
            .await?;
    let (mut sql, mut bindings) = permission.into_parts();
    let (ancestry_sql, ancestry_bindings) = node_history::scope_fence(&scope, Some(node_id))?;
    sql.extend(ancestry_sql);
    bindings.extend(ancestry_bindings);
    let slot = sql.len() + 1;
    sql.push("SELECT * OMIT id FROM node_log WHERE node_id=$resolve_node AND payload.editor_request_id=$resolve_request LIMIT 2".into());
    sql.push("SELECT * OMIT id FROM node WHERE node_id=$resolve_node AND tombstone=NONE".into());
    bindings.extend([
        ("resolve_node".into(), json!(node_id)),
        ("resolve_request".into(), json!(request_id)),
    ]);
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        sql.join("; ")
    ));
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut response = query.await?.check()?;
    let rows: Vec<Value> = response.take(slot)?;
    anyhow::ensure!(rows.len() <= 1, "ambiguous editor request receipt");
    let current_rows: Vec<Value> = response.take(slot + 1)?;
    anyhow::ensure!(current_rows.len() == 1, "object not found or not permitted");
    let current: NodeRowSnapshot =
        serde_json::from_value(node_history::normalize_snapshot(current_rows[0].clone())?)?;
    anyhow::ensure!(
        current.node_id == node_id && current.petal_id == petal_id,
        "unrelated current object"
    );
    let Some(row) = rows.first() else {
        return Ok((None, current));
    };
    anyhow::ensure!(
        row["payload"]["scope"] == scope,
        "request scope is no longer available"
    );
    anyhow::ensure!(
        row["payload"]["journal_version"] == node_history::JOURNAL_VERSION
            && matches!(row["op"].as_str(), Some("full_transform" | "property_set"))
            && row["row_version"].as_i64().is_some_and(|v| v > 0),
        "unsupported editor receipt fact"
    );
    let state: NodeRowSnapshot = serde_json::from_value(row["payload"]["after_state"].clone())?;
    anyhow::ensure!(
        state.node_id == node_id && state.petal_id == petal_id,
        "unrelated editor receipt"
    );
    anyhow::ensure!(
        super::crud::resolve_node_scope_handler(db, node_id)
            .await?
            .as_deref()
            == Some(scope.as_str()),
        "object moved during request reconciliation"
    );
    entity::prepare_scope_permission(db, &scope, local_did, fe_policy::RoleLevel::Viewer).await?;
    Ok((
        Some(NodeMutationReceipt {
            node_id: node_id.into(),
            history_version: row["row_version"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("invalid receipt version"))?,
            state,
            persist: PersistStatus::Committed,
        }),
        current,
    ))
}
