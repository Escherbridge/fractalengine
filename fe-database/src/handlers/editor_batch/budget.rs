//! Reserve unchanged-context Group inverses before the initial save; see AGENTS.md.

use super::{capture, creation, groups, prepare, FactIdentity, Provenance};
use crate::{handlers::field_def, types::OpLogEntry};
use fe_runtime::editor_batch::*;
use serde_json::{json, Value};

fn bytes(value: &impl serde::Serialize) -> anyhow::Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

fn larger<T: serde::Serialize + Clone>(a: &T, b: &T) -> anyhow::Result<T> {
    Ok(if bytes(a)? >= bytes(b)? {
        a.clone()
    } else {
        b.clone()
    })
}

fn membership(member: &EditorBatchMember) -> anyhow::Result<EditorMembershipValue> {
    Ok(match member.state.properties.get("view.groups") {
        None => EditorMembershipValue::Absent,
        Some(Value::Null) => EditorMembershipValue::Null,
        Some(value) => EditorMembershipValue::Groups(serde_json::from_value(value.clone())?),
    })
}

fn snapshot_bound(
    registry_omitted_base_bytes: usize,
    groups: &[fe_runtime::messages::VisibilityGroupDto],
) -> usize {
    const MAX_CBOR_HEAD: usize = 9;
    const FLOAT64: usize = 9;
    const BOOL: usize = 1;
    let keys = ["id", "name", "visible", "order"]
        .iter()
        .map(|key| MAX_CBOR_HEAD + key.len())
        .sum::<usize>();
    let registry_cbor = MAX_CBOR_HEAD
        + groups
            .iter()
            .map(|g| {
                MAX_CBOR_HEAD
                    + keys
                    + MAX_CBOR_HEAD
                    + g.id.len()
                    + MAX_CBOR_HEAD
                    + g.name.len()
                    + BOOL
                    + FLOAT64
            })
            .sum::<usize>();
    let property_cbor = MAX_CBOR_HEAD + MAX_CBOR_HEAD + "visibility_groups".len() + registry_cbor;
    registry_omitted_base_bytes + 4 * property_cbor.div_ceil(3) + 4
}

fn member_bound(
    before: &EditorBatchMember,
    after: &EditorBatchMember,
) -> anyhow::Result<EditorBatchMember> {
    let mut result = before.clone();
    result.state = larger(&before.state, &after.state)?;
    result.state.edit_seq = i64::MIN;
    result.history_version = Some(i64::MAX);
    if let (Some(old), Some(new)) = (&before.stamp_path, &after.stamp_path) {
        let path = result.stamp_path.as_mut().unwrap();
        path.state = larger(&old.state, &new.state)?;
        path.state.edit_seq = i64::MIN;
        path.history_version = Some(i64::MAX);
    }
    Ok(result)
}

pub(super) fn reserve(
    before: &EditorBatchCapture,
    after: &EditorBatchReceipt,
    maximum_entry: &OpLogEntry,
    sql: &[String],
    bindings: &[(String, Value)],
    actor: &str,
    petal_budget_base_bytes: usize,
) -> anyhow::Result<usize> {
    anyhow::ensure!(
        after
            .members
            .iter()
            .all(|m| m.history_version.is_none_or(|v| v <= i64::MAX - 2)),
        "Group save leaves no journal versions for its Undo and Redo"
    );
    let request = "\0".repeat(128);
    let all_groups: Vec<_> = before.groups.iter().chain(&after.groups).cloned().collect();
    let members = before
        .members
        .iter()
        .zip(&after.members)
        .map(|(old, new)| member_bound(old, new))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let group_id = after
        .created_group_id
        .clone()
        .or_else(|| match &before.target {
            EditorBatchTarget::Group { group_id } => Some(group_id.clone()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("Group lifecycle budget lacks its exact identity"))?;
    let mut capture = before.clone();
    capture.target = EditorBatchTarget::DeletedGroup {
        delete_request_id: request.clone(),
    };
    capture.members = members;
    capture.groups = all_groups.clone();
    let petal_snapshot_bytes = before
        .petal_snapshot
        .len()
        .max(snapshot_bound(petal_budget_base_bytes, &all_groups));
    capture.petal_snapshot = "X".repeat(petal_snapshot_bytes);
    capture.deleted_group = Some(EditorDeletedGroup {
        group_id: group_id.clone(),
        registry_before: all_groups.clone(),
        memberships: capture
            .members
            .iter()
            .map(membership)
            .collect::<anyhow::Result<_>>()?,
    });
    let mut receipt = after.clone();
    receipt.request_id = request.clone();
    receipt.target = capture.target.clone();
    receipt.members = capture.members.clone();
    receipt.groups = all_groups.clone();
    receipt.created_group_id = Some(group_id.clone());
    let provenance = Provenance {
        group_delete_before: Some(capture.clone()),
        group_create_before: Some(capture.clone()),
        group_undo_before: Some(capture.clone()),
        group_restore_of: Some(request.clone()),
        group_undo_of: Some(request.clone()),
        group_redo_of: Some(request.clone()),
    };
    let manifest = super::manifest_payload(&receipt, actor, &provenance);
    let mut future_entry = maximum_entry.clone();
    future_entry.payload = manifest.clone();
    let rules: Vec<Value> = bindings
        .iter()
        .find(|(name, _)| name.ends_with("_rules"))
        .map(|(_, value)| serde_json::from_value(value.clone()))
        .transpose()?
        .unwrap_or_default();
    let mut schema_versions = json!({});
    for member in before.members.iter().chain(&after.members) {
        if let Some(value) = member.state.properties.get("view.groups") {
            let versions = field_def::validate_captured_property_rules(
                &before.scope,
                "view.groups",
                value,
                &rules,
            )?;
            schema_versions = larger(&schema_versions, &versions)?;
        }
    }
    let mut future_sql: Vec<String> = sql
        .iter()
        .filter(|statement| {
            statement.contains("$entity_")
                || statement.contains("$batch_petal_snapshot")
                || (statement.contains("$batch_member_") && statement.contains("_rules"))
        })
        .cloned()
        .collect();
    future_sql.extend([
        super::MANIFEST_GUARD.into(),
        prepare::REQUEST_GUARD.into(),
        prepare::REGISTRY_WRITE.into(),
        prepare::REGISTRY_CHECK.into(),
        creation::proof_guard("budget_future_creation_origin"),
        creation::proof_guard("budget_future_creation_undo"),
    ]);
    for (marker, prefix) in [
        ("group_undo_of", "budget_future_creation_origin"),
        ("group_redo_of", "budget_future_creation_undo"),
    ] {
        let select = creation::consumption_select(marker, prefix);
        future_sql.push(format!(
            "IF array::len(({select})) != 0 {{ THROW 'Group lifecycle proof already consumed'; }}"
        ));
    }
    future_sql.extend([
        format!(
            "IF ({}) != [$deleted_payload] {{ THROW 'checked Group deletion proof changed'; }}",
            groups::PROOF
        ),
        format!(
            "IF array::len(({})) != 0 {{ THROW 'checked Group deletion proof already consumed'; }}",
            groups::CONSUMED
        ),
        groups::complete_member_guard(),
        format!(
            "IF array::len(({})) != 0 {{ THROW 'group gained uncaptured members'; }}",
            prepare::OUTSIDE_GROUP_MEMBERS_SQL
        ),
    ]);
    let retained =
        crate::handlers::visibility_groups::retained_group_select("budget_future_retained_group");
    future_sql.push(format!(
        "IF array::len(({retained})) != 0 {{ THROW 'group ID has retained memberships'; }}"
    ));
    let future_proofs = [
        ("budget_future_creation_origin_payload", &manifest),
        ("budget_future_creation_undo_payload", &manifest),
        ("budget_future_own_manifest_payload", &manifest),
    ];
    let mut future_bindings: Vec<(String, Value)> = bindings
        .iter()
        .filter(|(name, _)| {
            name.starts_with("entity_")
                || (name.starts_with("batch_member_")
                    && ["_rules", "_key", "_scopes"]
                        .iter()
                        .any(|suffix| name.ends_with(suffix)))
        })
        .cloned()
        .collect();
    future_bindings.extend([
        ("budget_future_registry".into(), json!(all_groups)),
        (
            "budget_future_petal_snapshot".into(),
            json!(capture.petal_snapshot),
        ),
        ("budget_future_own_request".into(), json!(request)),
        (
            "budget_future_petal_identity".into(),
            json!(before.petal_id),
        ),
    ]);
    for prefix in [
        "budget_future_creation_origin",
        "budget_future_creation_undo",
        "budget_future_deleted_origin",
    ] {
        future_bindings.extend([
            (format!("{prefix}_request"), json!(request)),
            (format!("{prefix}_current"), json!(request)),
            (format!("{prefix}_scope"), json!(before.scope)),
            (format!("{prefix}_actor"), json!(actor)),
            (format!("{prefix}_petal"), json!(before.petal_id)),
            (format!("{prefix}_group"), json!(group_id)),
            (
                format!("{prefix}_members"),
                json!(before
                    .members
                    .iter()
                    .map(|m| &m.node_id)
                    .collect::<Vec<_>>()),
            ),
        ]);
    }
    let max_clock = chrono::DateTime::<chrono::Utc>::MAX_UTC
        .to_rfc3339()
        .len()
        .max(chrono::DateTime::<chrono::Utc>::MIN_UTC.to_rfc3339().len());
    for (index, member) in capture.members.iter().enumerate() {
        let state = serde_json::to_value(&member.state)?;
        let payload = json!({"journal_version":1,"scope":before.scope,"data":{"key":"view.groups","value":member.state.properties.get("view.groups")},"after_state":state,"state":state,"coverage_reason":"legacy_or_uncovered_interval","schema_versions":schema_versions,"atomic_save_id":request,"atomic_save_request_id":request,"editor_request_id":request});
        let fact = super::fact_envelope(
            &member.node_id,
            "property_deleted",
            i64::MIN,
            &payload,
            actor,
            FactIdentity {
                log_id: "X".repeat(26),
                created_at: "X".repeat(max_clock),
                hlc_timestamp: i64::MIN,
            },
        );
        let old_raw = bindings
            .iter()
            .find(|(name, _)| name == &format!("batch_node_{index}_before"))
            .and_then(|(_, value)| value.as_array())
            .and_then(|rows| rows.first())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut raw = old_raw;
        raw.as_object_mut()
            .unwrap()
            .extend(state.as_object().unwrap().clone());
        let node = capture::StoredNode {
            state: state.clone(),
            raw,
            previous: Some(fact.clone()),
        };
        capture::fence(
            &node,
            &format!("budget_future_node_{index}"),
            &mut future_sql,
            &mut future_bindings,
        );
        if let Some(path) = &member.stamp_path {
            if !before
                .members
                .iter()
                .any(|m| m.node_id == path.state.node_id)
            {
                if let Some(binding) = bindings
                    .iter()
                    .find(|(name, _)| name == &format!("batch_path_{index}_history"))
                {
                    future_bindings.push(binding.clone());
                }
            }
            let state = serde_json::to_value(&path.state)?;
            let mut path_fact = fact.clone();
            path_fact["node_id"] = json!(path.state.node_id);
            path_fact["payload"]["after_state"] = state.clone();
            path_fact["payload"]["state"] = state.clone();
            let mut raw = bindings
                .iter()
                .find(|(name, _)| name == &format!("batch_path_{index}_before"))
                .and_then(|(_, value)| value.as_array())
                .and_then(|rows| rows.first())
                .cloned()
                .unwrap_or_else(|| json!({}));
            raw.as_object_mut()
                .unwrap()
                .extend(state.as_object().unwrap().clone());
            let node = capture::StoredNode {
                state: state.clone(),
                raw,
                previous: Some(path_fact),
            };
            capture::fence(
                &node,
                &format!("budget_future_path_{index}"),
                &mut future_sql,
                &mut future_bindings,
            );
        }
        let (writes, values) = prepare::reserve_materialization(&state, index);
        future_sql.extend(writes);
        future_bindings.extend(values);
        future_sql.push(format!(
            "CREATE node_log CONTENT $budget_future_fact_{index}"
        ));
        future_bindings.push((format!("budget_future_fact_{index}"), fact));
    }
    let reserved_bytes = bytes(&(
        "BEGIN TRANSACTION;\nCREATE op_log CONTENT $__op_entry;\nCOMMIT TRANSACTION;",
        future_entry,
        future_sql,
        future_proofs,
        future_bindings,
    ))?;
    anyhow::ensure!(reserved_bytes <= MAX_EDITOR_BATCH_BYTES, "Group save exceeds the complete 4 MiB Undo/Redo lifecycle reservation; use fewer objects or smaller stored property bags");
    Ok(reserved_bytes)
}
