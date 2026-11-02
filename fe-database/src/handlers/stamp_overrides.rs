//! Atomic path-locked stamp editing; see handlers/AGENTS.md section stamp-overrides.

use super::{crud, entity, field_def, node_history, preconditions};
use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::{
    messages::StampOverrideEdit,
    node_history::{NodeMutationReceipt, PersistStatus},
};
use serde_json::{json, Value};

pub(crate) struct PreparedStampEdit {
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    facts: Vec<(String, i64, Value)>,
    receipt: NodeMutationReceipt,
    entry: OpLogEntry,
    actor: String,
    request_id: String,
}

/// Capture every read that contributes to the batch before any effect.
pub(crate) async fn prepare(
    db: &Db,
    edit: &StampOverrideEdit,
    request_id: &str,
    actor: &str,
) -> anyhow::Result<PreparedStampEdit> {
    anyhow::ensure!(
        edit.instance_index < 4096,
        "stamp index exceeds the sampler bound"
    );
    anyhow::ensure!(
        !request_id.is_empty() && request_id.len() <= 128,
        "invalid stamp request identity"
    );
    anyhow::ensure!(
        edit.scale
            .iter()
            .all(|v| v.is_finite() && (0.01..=10_000.0).contains(v)),
        "stamp scale must be finite and 0.01..=10000"
    );
    anyhow::ensure!(
        edit.rotation.iter().all(|v| v.is_finite())
            && (edit
                .rotation
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                - 1.0)
                .abs()
                <= 0.0001,
        "stamp rotation must be a finite unit quaternion"
    );
    anyhow::ensure!(
        edit.arc_m.is_finite() && (0.0..=1_000_000.0).contains(&edit.arc_m),
        "stamp slide must be finite and 0..=1000000 meters"
    );
    let scope = crud::resolve_node_scope_handler(db, &edit.node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("stamp unavailable"))?;
    anyhow::ensure!(scope == edit.scope, "stamp ancestry changed");
    let permission =
        entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Editor).await?;
    let (mut statements, mut bindings) = permission.into_parts();
    let mut read = db.query(format!("SELECT * OMIT id FROM node WHERE node_id=$stamp; SELECT * OMIT id FROM node WHERE node_id=$path; SELECT *, {} AS stamp_petal_snapshot FROM petal WHERE petal_id=$petal", preconditions::PETAL_TYPED_SNAPSHOT_SQL))
        .bind(("stamp",edit.node_id.clone())).bind(("path",edit.path_id.clone()))
        .bind(("petal",crate::parse_scope(&scope)?.petal_id.ok_or_else(|| anyhow::anyhow!("missing petal"))?)).await?.check()?;
    let nodes: Vec<Value> = read.take(0)?;
    let paths: Vec<Value> = read.take(1)?;
    let petals: Vec<Value> = read.take(2)?;
    anyhow::ensure!(
        nodes.len() == 1 && paths.len() == 1 && petals.len() == 1,
        "stamp, path or petal unavailable"
    );
    let raw = nodes[0].clone();
    let path = paths[0].clone();
    let petal = &petals[0];
    let petal_snapshot = preconditions::petal_typed_snapshot(petal)?;
    anyhow::ensure!(
        raw.get("tombstone").is_none_or(Value::is_null)
            && path.get("tombstone").is_none_or(Value::is_null),
        "stamp or path deleted"
    );
    anyhow::ensure!(
        raw["properties"] == edit.before_properties
            && raw["properties"]["node_kind"] == "stamp"
            && raw["properties"]["path_id"] == edit.path_id
            && raw["properties"]["owning_path_id"] == edit.path_id
            && raw["properties"]["instance_index"].as_u64() == Some(edit.instance_index as u64)
            && raw["petal_id"] == path["petal_id"]
            && raw["petal_id"] == petal["petal_id"],
        "stamp identity or properties changed"
    );
    anyhow::ensure!(
        crud::resolve_node_scope_handler(db, &edit.path_id)
            .await?
            .as_deref()
            == Some(scope.as_str()),
        "stamp path belongs to another scope"
    );
    let anchors = fe_sdk::path_geometry::decode_anchor_rows(&path["properties"]["gpx_points"])
        .ok_or_else(|| anyhow::anyhow!("path coordinates or handles are invalid"))?;
    anyhow::ensure!(
        !anchors.is_empty() && anchors == edit.path_anchors,
        "path coordinates or handles changed since stamp editing"
    );
    let points = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    let descriptor =
        fe_sdk::path_asset::PathAssetDescriptor::from_json(&path["properties"]["path_asset"])
            .map_err(|error| anyhow::anyhow!("path stamp descriptor unavailable: {error}"))?;
    anyhow::ensure!(
        descriptor.to_json() == edit.path_descriptor,
        "path stamp descriptor changed"
    );
    let scale = petal["terrain"]["world_scale"]
        .as_f64()
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    let length = fe_sdk::path_geometry::polyline_length(&points) / scale;
    anyhow::ensure!(
        length.is_finite() && edit.arc_m <= length + 0.0001,
        "stamp slide exceeds current flattened path length in meters"
    );
    let previous = node_history::latest(db, &edit.node_id).await?;
    let path_previous = node_history::latest(db, &edit.path_id).await?;
    anyhow::ensure!(
        edit.history_version.is_none()
            || edit.history_version == previous.as_ref().and_then(|r| r["row_version"].as_i64()),
        "stamp version changed"
    );
    anyhow::ensure!(
        edit.path_version.is_none()
            || edit.path_version
                == path_previous
                    .as_ref()
                    .and_then(|r| r["row_version"].as_i64()),
        "path version changed"
    );
    let (ancestry, ancestry_bindings) = node_history::scope_fence(&scope, Some(&edit.node_id))?;
    statements.extend(ancestry);
    bindings.extend(ancestry_bindings);
    let node_state = node_history::normalize_snapshot(raw.clone())?;
    let path_state = node_history::normalize_snapshot(path.clone())?;
    let mut node_fence = raw.clone();
    node_fence.as_object_mut().unwrap().remove("position");
    let mut path_fence = path;
    path_fence.as_object_mut().unwrap().remove("position");
    statements.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$stamp_node) != $stamp_before { THROW 'stamp changed during save'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$stamp_node AND position=<geometry<point>> [$stamp_x,$stamp_z])) != 1 { THROW 'stamp position changed'; }".into(),
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$stamp_path) != $stamp_path_before { THROW 'path changed during save'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$stamp_path AND position=<geometry<point>> [$stamp_path_x,$stamp_path_z])) != 1 { THROW 'path position changed'; }".into(),
        format!("IF (SELECT VALUE {} FROM petal WHERE petal_id=$stamp_petal) != $stamp_petal_before {{ THROW 'map changed during save'; }}", preconditions::PETAL_TYPED_SNAPSHOT_SQL),
        "IF (SELECT * OMIT id FROM node_log WHERE node_id=$stamp_node ORDER BY row_version DESC LIMIT 1) != $stamp_history { THROW 'stamp version changed during save'; }".into(),
        "IF (SELECT * OMIT id FROM node_log WHERE node_id=$stamp_path ORDER BY row_version DESC LIMIT 1) != $stamp_path_history { THROW 'path version changed during save'; }".into(),
        "IF array::len((SELECT VALUE log_id FROM node_log WHERE node_id=$stamp_node AND payload.stamp_request_id=$stamp_request)) != 0 { THROW 'stamp request already committed'; }".into(),
    ]);
    bindings.extend([
        ("stamp_node".into(), json!(edit.node_id)),
        ("stamp_path".into(), json!(edit.path_id)),
        ("stamp_before".into(), json!([node_fence])),
        ("stamp_x".into(), node_state["position"][0].clone()),
        ("stamp_z".into(), node_state["position"][2].clone()),
        ("stamp_path_x".into(), path_state["position"][0].clone()),
        ("stamp_path_z".into(), path_state["position"][2].clone()),
        ("stamp_path_before".into(), json!([path_fence])),
        ("stamp_petal".into(), raw["petal_id"].clone()),
        ("stamp_petal_before".into(), json!([petal_snapshot])),
        (
            "stamp_history".into(),
            json!(previous.iter().collect::<Vec<_>>()),
        ),
        (
            "stamp_path_history".into(),
            json!(path_previous.iter().collect::<Vec<_>>()),
        ),
        ("stamp_request".into(), json!(request_id)),
    ]);
    let values = [
        ("stamp.override.scale", json!(edit.scale)),
        ("stamp.override.rotation", json!(edit.rotation)),
        ("stamp.override.arc_m", json!(edit.arc_m)),
    ];
    let mut state = node_history::normalize_snapshot(raw)?;
    let mut prior = previous;
    let mut facts = Vec::new();
    let mut version = 0;
    for (index, (key, value)) in values.iter().enumerate() {
        let rules = field_def::capture_property_rules(db, &scope, key).await?;
        let schemas = field_def::validate_captured_property_rules(&scope, key, value, &rules)?;
        let name = format!("stamp_rule_{index}");
        let key_name = format!("stamp_key_{index}");
        let value_name = format!("stamp_value_{index}");
        statements.push(format!(
            "IF ({}) != ${name} {{ THROW 'stamp schema changed during save'; }}",
            field_def::PROPERTY_RULES_SQL
                .replace("$key", &format!("${key_name}"))
                .replace("$scopes", "$stamp_scopes")
        ));
        bindings.extend([
            (name, json!(rules)),
            (key_name.clone(), json!(key)),
            (value_name.clone(), value.clone()),
        ]);
        statements.push(format!(
            "UPDATE node SET properties[${key_name}]=${value_name} WHERE node_id=$stamp_node"
        ));
        let prepared = node_history::prepare_change(
            state,
            prior.as_ref(),
            &scope,
            "property_set",
            json!({"key":key,"value":value}),
            schemas,
        )?;
        version = prepared.version;
        state = prepared.after;
        prior = prepared
            .facts
            .last()
            .map(|(_, version, payload)| json!({"row_version":version,"payload":payload}));
        facts.extend(prepared.facts);
    }
    bindings.push((
        "stamp_scopes".into(),
        json!(field_def::property_rule_scopes(&scope)),
    ));
    anyhow::ensure!(
        serde_json::to_vec(&facts)?.len() <= 4 * 1024 * 1024,
        "stamp journal batch is too large"
    );
    let receipt = NodeMutationReceipt {
        node_id: edit.node_id.clone(),
        history_version: version,
        state: serde_json::from_value(state)?,
        persist: PersistStatus::Committed,
    };
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(edit.node_id.clone()),
        op_type: OpType::PropertySet,
        payload: json!({"node_id":edit.node_id,"stamp_overrides":values,"stamp_request_id":request_id}),
        sig: "00".repeat(64),
    };
    Ok(PreparedStampEdit {
        statements,
        bindings,
        facts,
        receipt,
        entry,
        actor: actor.into(),
        request_id: request_id.into(),
    })
}

/// Commit the captured permission, route, rows, schemas and ordinary facts together.
pub(crate) async fn commit(
    db: &Db,
    prepared: PreparedStampEdit,
) -> anyhow::Result<NodeMutationReceipt> {
    let PreparedStampEdit {
        mut statements,
        mut bindings,
        facts,
        receipt,
        entry,
        actor,
        request_id,
    } = prepared;
    crate::op_log::commit_operation_atomic_with(db,entry,|hlc| {
        let last=facts.len().saturating_sub(1);
        for (index,(op,version,mut payload)) in facts.into_iter().enumerate() {
            if op!="baseline" {payload["stamp_batch_id"]=json!(request_id);}
            if index==last {payload["stamp_request_id"]=json!(request_id);payload["editor_request_id"]=json!(request_id);}
            let name=format!("stamp_fact_{index}"); statements.push(format!("CREATE node_log CONTENT ${name}"));
            bindings.push((name,json!({"log_id":ulid::Ulid::new().to_string(),"node_id":receipt.node_id,"hlc_timestamp":hlc as i64,"op":op,"source_did":actor,
                "payload":payload,"row_version":version,"created_at":chrono::Utc::now().to_rfc3339()})));
        }
        (statements,bindings)
    }).await?;
    Ok(receipt)
}
