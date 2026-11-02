//! Checked GUI stamp promotion; see handlers/AGENTS.md section checked-stamp-promotion.

use super::{crud, entity, field_def, node_history};
use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::messages::CheckedStampPromotion;
use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) struct PreparedPromotion {
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    entry: OpLogEntry,
    node_id: String,
    properties: Value,
    existing: bool,
}

/// Capture source rows, role, ancestry, schemas and target absence before any effect.
pub(crate) async fn prepare(
    db: &Db,
    capture: &CheckedStampPromotion,
    request: &str,
    actor: &str,
) -> anyhow::Result<PreparedPromotion> {
    anyhow::ensure!(
        !request.is_empty() && request.len() <= 128,
        "invalid promotion request"
    );
    anyhow::ensure!(
        capture.instance_index < 4096,
        "stamp index exceeds sampler limit"
    );
    anyhow::ensure!(
        serde_json::to_vec(&capture.before_properties)?.len() <= 4 * 1024 * 1024,
        "stamp source capture exceeds limit"
    );
    let scope = crud::resolve_node_scope_handler(db, &capture.path_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("stamp source unavailable"))?;
    anyhow::ensure!(scope == capture.scope, "stamp source ancestry changed");
    let parsed = crate::parse_scope(&scope)?;
    anyhow::ensure!(
        parsed.petal_id.as_deref() == Some(capture.petal_id.as_str()),
        "stamp petal changed"
    );
    let permission =
        entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Editor).await?;
    let (mut statements, mut bindings) = permission.into_parts();
    let node_id = format!("{}#inst-{}", capture.path_id, capture.instance_index);
    let mut read = db.query("SELECT * OMIT id FROM node WHERE node_id=$path; SELECT * OMIT id FROM petal WHERE petal_id=$petal; SELECT * OMIT id FROM node WHERE node_id=$stamp")
        .bind(("path",capture.path_id.clone())).bind(("petal",capture.petal_id.clone())).bind(("stamp",node_id.clone())).await?.check()?;
    let paths: Vec<Value> = read.take(0)?;
    let petals: Vec<Value> = read.take(1)?;
    let targets: Vec<Value> = read.take(2)?;
    anyhow::ensure!(
        paths.len() == 1 && petals.len() == 1 && targets.len() <= 1,
        "stamp source or target is ambiguous"
    );
    let path = paths[0].clone();
    let petal = petals[0].clone();
    anyhow::ensure!(
        path.get("tombstone").is_none_or(Value::is_null)
            && path["petal_id"] == capture.petal_id
            && path["properties"] == capture.before_properties,
        "stamp source changed before promotion"
    );
    let scale = petal["terrain"]["world_scale"]
        .as_f64()
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    anyhow::ensure!(
        capture.world_scale.is_finite()
            && capture.world_scale > 0.0
            && capture.world_scale == scale,
        "stamp map scale changed"
    );
    let descriptor =
        fe_sdk::path_asset::PathAssetDescriptor::from_json(&path["properties"]["path_asset"])?;
    anyhow::ensure!(
        !descriptor.asset_path.trim().is_empty(),
        "stamp asset unavailable"
    );
    let anchors = fe_sdk::path_geometry::decode_anchor_rows(&path["properties"]["gpx_points"])
        .ok_or_else(|| anyhow::anyhow!("stamp route is invalid"))?;
    let points = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    let total = fe_sdk::path_geometry::polyline_length(&points);
    anyhow::ensure!(
        total.is_finite()
            && (capture.instance_index as usize)
                < fe_sdk::path_asset::sample_progresses(&points, &descriptor, total, scale).len(),
        "stamp index no longer exists"
    );
    let prior = node_history::latest(db, &capture.path_id).await?;
    anyhow::ensure!(
        capture.path_version.is_none()
            || capture.path_version == prior.as_ref().and_then(|row| row["row_version"].as_i64()),
        "stamp path version changed"
    );
    let (ancestry, ancestry_bindings) = node_history::scope_fence(&scope, Some(&capture.path_id))?;
    statements.extend(ancestry);
    bindings.extend(ancestry_bindings);
    let state = node_history::normalize_snapshot(path.clone())?;
    let mut path_fence = path;
    path_fence.as_object_mut().unwrap().remove("position");
    statements.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$promotion_path) != $promotion_path_before { THROW 'stamp source changed during promotion'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$promotion_path AND position=<geometry<point>> [$promotion_x,$promotion_z])) != 1 { THROW 'stamp source position changed'; }".into(),
        "IF (SELECT * OMIT id FROM petal WHERE petal_id=$promotion_petal) != $promotion_petal_before { THROW 'stamp map changed during promotion'; }".into(),
        "IF (SELECT * OMIT id FROM node_log WHERE node_id=$promotion_path ORDER BY row_version DESC LIMIT 1) != $promotion_history { THROW 'stamp source version changed during promotion'; }".into(),
    ]);
    bindings.extend([
        ("promotion_path".into(), json!(capture.path_id)),
        ("promotion_path_before".into(), json!([path_fence])),
        ("promotion_x".into(), state["position"][0].clone()),
        ("promotion_z".into(), state["position"][2].clone()),
        ("promotion_petal".into(), json!(capture.petal_id)),
        ("promotion_petal_before".into(), json!([petal])),
        (
            "promotion_history".into(),
            json!(prior.iter().collect::<Vec<_>>()),
        ),
        ("promotion_node".into(), json!(node_id)),
    ]);
    let existing = !targets.is_empty();
    let properties = if let Some(target) = targets.first() {
        anyhow::ensure!(
            target["petal_id"] == capture.petal_id
                && target.get("tombstone").is_none_or(Value::is_null)
                && target["properties"]["node_kind"] == "stamp"
                && target["properties"]["path_id"] == capture.path_id
                && target["properties"]["owning_path_id"] == capture.path_id
                && target["properties"]["instance_index"].as_u64()
                    == Some(u64::from(capture.instance_index)),
            "stamp node identity collision"
        );
        let target_state = node_history::normalize_snapshot(target.clone())?;
        let mut fence = target.clone();
        fence.as_object_mut().unwrap().remove("position");
        statements.push("IF (SELECT * OMIT id, position FROM node WHERE node_id=$promotion_node) != $promotion_existing { THROW 'promoted stamp changed'; }".into());
        statements.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$promotion_node AND position=<geometry<point>> [$promotion_existing_x,$promotion_existing_z])) != 1 { THROW 'promoted stamp position changed'; }".into());
        bindings.extend([
            ("promotion_existing".into(), json!([fence])),
            (
                "promotion_existing_x".into(),
                target_state["position"][0].clone(),
            ),
            (
                "promotion_existing_z".into(),
                target_state["position"][2].clone(),
            ),
        ]);
        target["properties"].clone()
    } else {
        statements.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$promotion_node)) != 0 { THROW 'stamp node appeared during promotion'; }".into());
        json!({"node_kind":"stamp","path_id":capture.path_id,"owning_path_id":capture.path_id,"instance_index":capture.instance_index})
    };
    if !existing {
        for (index, (key, value)) in properties.as_object().unwrap().iter().enumerate() {
            let rules = field_def::capture_property_rules(db, &scope, key).await?;
            field_def::validate_captured_property_rules(&scope, key, value, &rules)?;
            let key_name = format!("promotion_schema_key_{index}");
            let rule_name = format!("promotion_schema_{index}");
            statements.push(format!(
                "IF ({}) != ${rule_name} {{ THROW 'stamp creation schema changed'; }}",
                field_def::PROPERTY_RULES_SQL
                    .replace("$key", &format!("${key_name}"))
                    .replace("$scopes", "$promotion_scopes")
            ));
            bindings.extend([(key_name, json!(key)), (rule_name, json!(rules))]);
        }
        bindings.push((
            "promotion_scopes".into(),
            json!(field_def::property_rule_scopes(&scope)),
        ));
        statements.push("CREATE node CONTENT {node_id:$promotion_node,petal_id:$promotion_petal,display_name:$promotion_name,asset_id:NONE,position:<geometry<point>> [0.0,0.0],elevation:0.0,rotation:[0.0,0.0,0.0,1.0],scale:[1.0,1.0,1.0],interactive:true,created_at:$promotion_now,properties:$promotion_properties}".into());
        bindings.extend([
            (
                "promotion_name".into(),
                json!(format!("{} #{}", capture.path_id, capture.instance_index)),
            ),
            (
                "promotion_now".into(),
                json!(chrono::Utc::now().to_rfc3339()),
            ),
            ("promotion_properties".into(), properties.clone()),
        ]);
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: NodeId(node_id.clone()),
        op_type: OpType::NodePromoted,
        payload: json!({"node_id":node_id,"petal_id":capture.petal_id,"path_id":capture.path_id,"instance_index":capture.instance_index,"editor_request_id":request}),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    Ok(PreparedPromotion {
        statements,
        bindings,
        entry,
        node_id,
        properties,
        existing,
    })
}

/// Commit the sealed source/role/schema capture and return only confirmed properties.
pub(crate) async fn commit(
    db: &Db,
    prepared: PreparedPromotion,
) -> anyhow::Result<(String, Value, bool)> {
    let PreparedPromotion {
        statements,
        bindings,
        entry,
        node_id,
        properties,
        existing,
    } = prepared;
    if existing {
        let script = format!(
            "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
            statements.join(";\n")
        );
        let mut query = db.query(script);
        for (key, value) in bindings {
            query = query.bind((key, value));
        }
        query.await?.check()?;
    } else {
        crate::op_log::commit_operation_atomic_with(db, entry, |_| (statements, bindings)).await?;
    }
    Ok((node_id, properties, !existing))
}
