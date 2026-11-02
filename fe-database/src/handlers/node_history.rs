//! Atomic ordinary node journal and bounded replay; see src/AGENTS.md §node-history.

use serde_json::{json, Value};

use crate::{op_log::commit_operation_atomic_with, repo::Db, types::OpLogEntry};

const JOURNAL_VERSION: u64 = 1;
const MAX_REPLAY_ROWS: usize = 10_000;

fn check_snapshot_size(value: &Value) -> anyhow::Result<()> {
    if serde_json::to_vec(value)?.len() > fe_sdk::property_schema::MAX_PROPERTY_BYTES {
        anyhow::bail!("covered object snapshot exceeds 65536 bytes");
    }
    Ok(())
}

pub(crate) async fn snapshot(db: &Db, node_id: &str) -> anyhow::Result<Option<Value>> {
    let mut response = db
        .query("SELECT * OMIT id FROM node WHERE node_id = $id LIMIT 1")
        .bind(("id", node_id.to_string()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    rows.into_iter().next().map(normalize_snapshot).transpose()
}

fn normalize_snapshot(mut row: Value) -> anyhow::Result<Value> {
    let coordinates = row["position"]["coordinates"]
        .as_array()
        .filter(|c| c.len() == 2)
        .ok_or_else(|| anyhow::anyhow!("node position is not a point"))?;
    row["position"] = json!([coordinates[0], row["elevation"], coordinates[1]]);
    row.as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("node snapshot is not an object"))?
        .entry("properties")
        .or_insert_with(|| json!({}));
    if row["properties"].is_null() {
        row["properties"] = json!({});
    }
    Ok(row)
}

async fn latest(db: &Db, node_id: &str) -> anyhow::Result<Option<Value>> {
    let mut response = db
        .query(
            "SELECT * OMIT id FROM node_log WHERE node_id = $id ORDER BY row_version DESC LIMIT 1",
        )
        .bind(("id", node_id.to_string()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(rows.into_iter().next())
}

fn row_version(row: &Value) -> anyhow::Result<i64> {
    row["row_version"]
        .as_i64()
        .filter(|version| *version > 0)
        .ok_or_else(|| anyhow::anyhow!("invalid journal row_version"))
}

pub(crate) async fn committed_receipt(db: &Db, node_id: &str) -> anyhow::Result<Value> {
    let row = latest(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("committed journal row missing"))?;
    let state = snapshot(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("committed node missing"))?;
    if row["payload"]["journal_version"] != JOURNAL_VERSION
        || row["payload"]["after_state"] != state
    {
        anyhow::bail!("committed journal does not match stored state");
    }
    Ok(
        json!({"node_id":node_id,"history_version":row_version(&row)?,"state":state,"persist":"committed"}),
    )
}

pub(crate) async fn receipt(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<fe_runtime::node_history::NodeMutationReceipt> {
    Ok(serde_json::from_value(
        committed_receipt(db, node_id).await?,
    )?)
}

pub(crate) async fn read_reply(
    db: &Db,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
    correlation_id: Option<String>,
) -> fe_runtime::messages::DbResult {
    use fe_runtime::messages::DbResult;
    let result = async {
        let value = read(db, node_id, after_version, limit, at_version).await?;
        Ok::<_, anyhow::Error>(if at_version.is_some() {
            DbResult::NodeStateAtVersionLoaded {
                state: serde_json::from_value(value)?,
                correlation_id: correlation_id.clone(),
            }
        } else {
            DbResult::NodeHistoryLoaded {
                page: serde_json::from_value(value)?,
                correlation_id: correlation_id.clone(),
            }
        })
    }
    .await;
    result.unwrap_or_else(|error| {
        super::correlated_failure(
            &correlation_id,
            format!("Read node history failed: {error}"),
        )
    })
}

/// Convert authoritative readback into the one full GUI materialization boundary.
pub(crate) fn materialization(
    receipt: &fe_runtime::node_history::NodeMutationReceipt,
    asset_path: Option<String>,
) -> anyhow::Result<fe_runtime::messages::CommittedNodeMaterialization> {
    let state = &receipt.state;
    let rotation_euler: [f32; 3] = state
        .rotation
        .iter()
        .map(|&v| v as f32)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| anyhow::anyhow!("created object rotation is not Euler XYZ"))?;
    let rotation = bevy::math::Quat::from_euler(
        bevy::math::EulerRot::XYZ,
        rotation_euler[0],
        rotation_euler[1],
        rotation_euler[2],
    )
    .to_array();
    Ok(fe_runtime::messages::CommittedNodeMaterialization {
        node: fe_runtime::messages::NodeDto {
            node_id: state.node_id.clone(),
            petal_id: state.petal_id.clone(),
            name: state
                .display_name
                .clone()
                .ok_or_else(|| anyhow::anyhow!("created object name missing"))?,
            position: state.position.map(|v| v as f32),
            rotation,
            scale: state.scale.map(|v| v as f32),
            has_asset: state.asset_id.is_some(),
            asset_path,
        },
        rotation_euler,
        asset_id: state.asset_id.clone(),
        properties: Value::Object(state.properties.clone()),
        history_version: receipt.history_version,
    })
}

/// Atomically create an ordinary object and its complete first journal baseline.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_object(
    db: &Db,
    petal_id: &str,
    name: &str,
    asset_id: Option<&str>,
    position: [f32; 3],
    rotation: Vec<f64>,
    scale: Vec<f64>,
    properties: serde_json::Map<String, Value>,
    actor: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let scope = super::preconditions::require_petal_scope(db, petal_id, "CreateObject").await?;
    if name.trim().is_empty()
        || name.len() > 1024
        || position.iter().any(|v| !v.is_finite())
        || rotation.iter().chain(&scale).any(|v| !v.is_finite())
    {
        anyhow::bail!("object requires a non-empty bounded name and finite transform");
    }
    let mut schema_versions = serde_json::Map::new();
    for (key, value) in &properties {
        if let Value::Object(versions) =
            super::field_def::validate_property_in_scope(db, &scope, key, value).await?
        {
            schema_versions.extend(versions);
        }
    }
    let asset_path = if let Some(asset_id) = asset_id {
        let mut response = db
            .query("SELECT content_hash, content_type FROM asset WHERE asset_id = $id LIMIT 1")
            .bind(("id", asset_id.to_string()))
            .await?
            .check()?;
        let rows: Vec<Value> = response.take(0)?;
        let row = rows
            .first()
            .ok_or_else(|| anyhow::anyhow!("CreateObject matched no asset"))?;
        let hash = row["content_hash"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("scene asset has no content hash"))?;
        let ext = match row["content_type"].as_str() {
            Some("model/gltf-binary") => "glb",
            Some("model/gltf+json") => "gltf",
            _ => anyhow::bail!("asset is not a scene asset"),
        };
        Some(format!("blob://{hash}.{ext}"))
    } else {
        None
    };
    let id = ulid::Ulid::new().to_string();
    let state = json!({"node_id":id,"petal_id":petal_id,"display_name":name,
        "position":position,"elevation":position[1] as f64,"rotation":rotation,"scale":scale,
        "properties":properties,"interactive":asset_id.is_some(),"edit_seq":0,
        "created_at":chrono::Utc::now().to_rfc3339()});
    let mut state = state;
    if let Some(asset) = asset_id {
        state["asset_id"] = json!(asset);
    }
    check_snapshot_size(&state)?;
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: crate::types::NodeId(id.clone()),
        op_type: crate::types::OpType::NodeCreated,
        payload: state.clone(),
        sig: "00".repeat(64),
    };
    let actor = actor.to_string();
    let created_id = id.clone();
    commit_operation_atomic_with(db, entry, move |hlc| {
        let fact = json!({"log_id":ulid::Ulid::new().to_string(),"node_id":created_id,
            "hlc_timestamp":hlc as i64,"op":"baseline","source_did":actor,"row_version":1,
            "created_at":chrono::Utc::now().to_rfc3339(),"payload":{"journal_version":JOURNAL_VERSION,
                "scope":scope,"state":state,"after_state":state,"coverage_reason":"object_created",
                "schema_versions":schema_versions}});
        (vec!["CREATE node CONTENT object::extend($created_state, {position: <geometry<point>> [$created_x, $created_z]})".into(),
            "CREATE node_log CONTENT $created_fact".into()],
            vec![("created_state".into(),state),("created_fact".into(),fact),
                ("created_x".into(),json!(position[0] as f64)),("created_z".into(),json!(position[2] as f64))])
    }).await?;
    Ok((id, asset_path))
}

/// Commit the materialized change, intention, baseline and journal facts together.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_mutation(
    db: &Db,
    entry: OpLogEntry,
    statements: Vec<String>,
    mut bindings: Vec<(String, Value)>,
    actor: &str,
    op: &str,
    data: Value,
    schema_versions: Value,
) -> anyhow::Result<i64> {
    let node_id = entry.node_id.0.clone();
    let before = snapshot(db, &node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node not found"))?;
    let scope = super::crud::resolve_node_scope_handler(db, &node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node scope not found"))?;
    let previous = latest(db, &node_id).await?;
    let last_version = previous.as_ref().map(row_version).transpose()?.unwrap_or(0);
    let continuous = previous.as_ref().is_some_and(|row| {
        row["payload"]["journal_version"] == JOURNAL_VERSION
            && row["payload"]["after_state"] == before
    });
    let baseline_version = last_version
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("node version exhausted"))?;
    let version = baseline_version
        .checked_add(i64::from(!continuous))
        .ok_or_else(|| anyhow::anyhow!("node version exhausted"))?;
    let mut after = before.clone();
    reduce_change(&mut after, op, &data)?;
    if op == "full_transform" {
        after["edit_seq"] = json!(before["edit_seq"]
            .as_i64()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transform edit sequence exhausted"))?);
        after["elevation"] = data["position"][1].clone();
    }
    check_snapshot_size(&before)?;
    check_snapshot_size(&after)?;
    let mut facts = Vec::new();
    if !continuous {
        facts.push(("baseline", baseline_version, json!({
            "journal_version": JOURNAL_VERSION, "scope": scope,
            "after_state": before, "state": before,
            "coverage_reason": if previous.is_none() { "first_covered_mutation" } else { "legacy_or_uncovered_interval" }
        })));
    }
    facts.push((
        op,
        version,
        json!({"journal_version": JOURNAL_VERSION,
        "scope": scope, "data": data, "after_state": after, "schema_versions": schema_versions}),
    ));
    let actor = actor.to_string();
    commit_operation_atomic_with(db, entry, move |hlc| {
        let mut sql = statements;
        // Optimistic backstop for the documented single DB-thread writer.
        sql.insert(0, "IF array::len((SELECT VALUE node_id FROM node WHERE node_id = $history_node)) != 1 { THROW 'history target disappeared'; }".into());
        bindings.push(("history_node".into(), json!(node_id)));
        for (index, (op, row_version, payload)) in facts.into_iter().enumerate() {
            let name = format!("history_fact_{index}");
            sql.push(format!("CREATE node_log CONTENT ${name}"));
            bindings.push((name, json!({"log_id": ulid::Ulid::new().to_string(),
                "node_id": node_id, "hlc_timestamp": hlc as i64, "op": op,
                "source_did": actor, "payload": payload, "row_version": row_version,
                "created_at": chrono::Utc::now().to_rfc3339()})));
        }
        (sql, bindings)
    }).await?;
    Ok(version)
}

fn reduce_change(state: &mut Value, op: &str, data: &Value) -> anyhow::Result<()> {
    match op {
        "property_set" => {
            let key = data["key"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("journal property key missing"))?;
            let value = data
                .get("value")
                .ok_or_else(|| anyhow::anyhow!("journal property value missing"))?;
            state["properties"]
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("journal properties are not an object"))?
                .insert(key.to_string(), value.clone());
        }
        "property_deleted" => {
            let key = data["key"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("journal property key missing"))?;
            state["properties"]
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("journal properties are not an object"))?
                .remove(key);
        }
        "full_transform" => {
            for key in ["position", "rotation", "scale"] {
                let array = data[key]
                    .as_array()
                    .filter(|a| a.len() == 3 && a.iter().all(Value::is_number))
                    .ok_or_else(|| anyhow::anyhow!("invalid journal transform {key}"))?;
                state[key] = json!(array);
            }
        }
        _ => anyhow::bail!("unsupported journal operation {op}"),
    }
    Ok(())
}

/// Read a bounded immutable page or reconstruct one exact commit version.
pub(crate) async fn read(
    db: &Db,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
) -> anyhow::Result<Value> {
    if after_version < 0 || !(1..=200).contains(&limit) || at_version.is_some_and(|v| v < 1) {
        anyhow::bail!("invalid history bounds");
    }
    let latest = latest(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("history unavailable"))?;
    let current_version = row_version(&latest)?;
    let mut baseline_response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND op = 'baseline' AND payload.journal_version = 1 ORDER BY row_version ASC LIMIT 1")
        .bind(("id", node_id.to_string())).await?.check()?;
    let baselines: Vec<Value> = baseline_response.take(0)?;
    let coverage_start = baselines.first().map(row_version).transpose()?;
    let mut metadata = json!({"node_id":node_id, "journal_version": JOURNAL_VERSION,
        "current_version":current_version, "coverage_start_version":coverage_start,
        "coverage_started_at":baselines.first().and_then(|b|b.get("created_at")),
        "supported_operations":["baseline","property_set","property_deleted","full_transform"],
        "order":"per_object_commit_version", "event_time_ordering":false});
    let current_matches = snapshot(db, node_id).await?.is_some_and(|state| {
        latest["payload"]["journal_version"] == JOURNAL_VERSION
            && latest["payload"]["after_state"] == state
    });
    metadata["current_state_verified"] = json!(current_matches);
    if let Some(target) = at_version {
        return replay(db, node_id, target, metadata).await;
    }
    let mut response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND row_version > $after ORDER BY row_version ASC LIMIT $limit")
        .bind(("id", node_id.to_string())).bind(("after", after_version)).bind(("limit", (limit + 1) as i64)).await?.check()?;
    let mut rows: Vec<Value> = response.take(0)?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    metadata["next_after_version"] = if more {
        rows.last()
            .map(row_version)
            .transpose()?
            .map_or(Value::Null, |v| json!(v))
    } else {
        Value::Null
    };
    metadata["entries"] = json!(rows);
    Ok(metadata)
}

async fn replay(db: &Db, node_id: &str, target: i64, mut metadata: Value) -> anyhow::Result<Value> {
    metadata["at_version"] = json!(target);
    let mut response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND op = 'baseline' AND payload.journal_version = 1 AND row_version <= $target ORDER BY row_version DESC LIMIT 1")
        .bind(("id", node_id.to_string())).bind(("target", target)).await?.check()?;
    let baselines: Vec<Value> = response.take(0)?;
    let Some(baseline) = baselines.first() else {
        metadata["availability"] = json!("before_coverage");
        return Ok(metadata);
    };
    let start = row_version(baseline)?;
    let mut state = baseline["payload"]["state"].clone();
    let mut expected = start;
    let mut processed = 0;
    while expected < target {
        let mut response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND row_version > $start AND row_version <= $target ORDER BY row_version ASC LIMIT 200 TIMEOUT 5s")
            .bind(("id",node_id.to_string())).bind(("start",expected)).bind(("target",target)).await?.check()?;
        let rows: Vec<Value> = response.take(0)?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            processed += 1;
            expected = expected
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("version exhausted"))?;
            if processed > MAX_REPLAY_ROWS
                || row_version(row)? != expected
                || row["payload"]["journal_version"] != JOURNAL_VERSION
            {
                metadata["availability"] = json!("incomplete_interval");
                return Ok(metadata);
            }
            if reduce_change(
                &mut state,
                row["op"].as_str().unwrap_or(""),
                &row["payload"]["data"],
            )
            .is_err()
            {
                metadata["availability"] = json!("unsupported_operation");
                return Ok(metadata);
            }
            if row["op"] == "full_transform" {
                state["edit_seq"] =
                    json!(state["edit_seq"]
                        .as_i64()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("transform edit sequence exhausted"))?);
                state["elevation"] = state["position"][1].clone();
            }
            if state != row["payload"]["after_state"] {
                metadata["availability"] = json!("incomplete_interval");
                return Ok(metadata);
            }
        }
    }
    if expected != target {
        metadata["availability"] = json!("version_unavailable");
    } else {
        metadata["availability"] = json!("available");
        metadata["state"] = state;
        metadata["baseline_version"] = json!(start);
    }
    Ok(metadata)
}
