//! Typed plugin reads, atomic batches and tick flushes; see src/AGENTS.md §plugin-host.

use super::{entity, field_def, node_history};
use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use bevy::math::{EulerRot, Quat};
use fe_runtime::messages::{IotReadingInput, SceneChange};
use fe_runtime::plugin_host::{PluginAdmission, PluginRead, PluginRefusal, PluginReply};
use fe_sdk::manifest_v2::SceneWrite;
use fe_sdk::plugin_host::{
    PendingOp, StagedReading, StagedTimestamp, TransactionReceipt, MAX_HOST_BYTES,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const PLUGIN_QUERY_CANDIDATES: usize = 1024;

/// Tag a failure with its typed refusal so the reply keeps its WIT class.
fn refuse(kind: fn(String) -> PluginRefusal, detail: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(kind(detail.into()))
}

/// Classify a failure for the typed reply; an untagged failure before effects is an input refusal.
pub(crate) fn refusal(error: anyhow::Error) -> PluginRefusal {
    error
        .downcast::<PluginRefusal>()
        .unwrap_or_else(|error| PluginRefusal::InvalidInput(format!("{error:#}")))
}

async fn authorize_as(
    db: &Db,
    admission: &PluginAdmission,
    local_did: &str,
    floor: fe_policy::RoleLevel,
    granted: bool,
    handshake: bool,
) -> anyhow::Result<(String, entity::ParentMutation)> {
    admission
        .require_active()
        .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
    if !granted {
        return Err(refuse(
            PluginRefusal::CapabilityDenied,
            "plugin capability denied",
        ));
    }
    let scope = super::preconditions::require_petal_scope(db, admission.petal_id(), "PluginHost")
        .await
        .map_err(|error| refuse(PluginRefusal::Revoked, format!("{error:#}")))?;
    let parts = crate::parse_scope(&scope)?;
    if parts.petal_id.as_deref() != Some(admission.petal_id()) {
        return Err(refuse(PluginRefusal::Revoked, "plugin scope mismatch"));
    }
    if !admission.allows_scope(&crate::build_scope(&parts.verse_id, None, None), &scope) {
        return Err(refuse(PluginRefusal::CapabilityDenied, "plugin scope denied"));
    }
    if admission.bound_scope() != Some(scope.as_str())
        && !(handshake && admission.bound_scope().is_none())
    {
        return Err(refuse(
            PluginRefusal::Revoked,
            "plugin admitted ancestry changed",
        ));
    }
    let permission = entity::prepare_scope_permission(db, &scope, local_did, floor)
        .await
        .map_err(|error| refuse(PluginRefusal::Revoked, format!("{error:#}")))?;
    admission
        .require_active()
        .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
    Ok((scope, permission))
}

fn snapshot_for_plugin(
    raw: Value,
    admission: &PluginAdmission,
) -> anyhow::Result<fe_sdk::NodeSnapshot> {
    let normalized = node_history::normalize_snapshot(raw)?;
    let state: fe_runtime::node_history::NodeRowSnapshot = serde_json::from_value(normalized)?;
    anyhow::ensure!(
        state.petal_id == admission.petal_id() && state.tombstone.is_none(),
        "node not found or not permitted"
    );
    let rotation = fe_runtime::node_history::stored_rotation_quaternion(&state.rotation)
        .ok_or_else(|| anyhow::anyhow!("invalid stored node rotation"))?
        .map(f64::from);
    Ok(fe_sdk::NodeSnapshot {
        node_id: state.node_id,
        petal_id: state.petal_id,
        name: state.display_name.unwrap_or_default(),
        position: state.position,
        rotation,
        scale: state.scale,
        properties: state
            .properties
            .into_iter()
            .filter(|(key, _)| admission.allows_property(key))
            .map(|(key, value)| (key, fe_sdk::plugin_host::json_property(value)))
            .collect(),
    })
}

pub(crate) async fn read(
    db: &Db,
    admission: &PluginAdmission,
    request: PluginRead,
    local_did: &str,
) -> anyhow::Result<PluginReply> {
    let handshake = matches!(request, PluginRead::PetalScope);
    // A writer without scene reads (a readings-only adapter included) still pins its ancestry.
    let (floor, granted) = if handshake && !admission.allows(fe_sdk::CAP_STORAGE_READ) {
        (
            fe_policy::RoleLevel::Editor,
            admission.allows(fe_sdk::CAP_STORAGE_WRITE) || admission.readings().is_some(),
        )
    } else {
        (
            fe_policy::RoleLevel::Viewer,
            admission.allows(fe_sdk::CAP_STORAGE_READ),
        )
    };
    let (scope, permission) =
        authorize_as(db, admission, local_did, floor, granted, handshake).await?;
    if matches!(request, PluginRead::PetalScope) {
        permission.commit(db, &[], vec![]).await?;
        admission
            .require_active()
            .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
        return Ok(PluginReply::Scope(scope));
    }
    let (mut guards, mut bindings) = permission.into_parts();
    let mut sql = "SELECT * OMIT id FROM node WHERE petal_id=$plugin_read_petal AND tombstone=NONE"
        .to_string();
    bindings.push(("plugin_read_petal".into(), json!(admission.petal_id())));
    let is_single = matches!(request, PluginRead::Node(_));
    match &request {
        PluginRead::Node(id) => {
            fe_sdk::storage::validate_node_id(id).map_err(anyhow::Error::msg)?;
            sql.push_str(" AND node_id=$plugin_read_node");
            bindings.push(("plugin_read_node".into(), json!(id)));
            sql.push_str(" LIMIT 2");
        }
        PluginRead::Nodes(query) => {
            if !admission.allows(fe_sdk::CAP_QUERY_SELECT) {
                return Err(refuse(
                    PluginRefusal::CapabilityDenied,
                    "plugin query capability denied",
                ));
            }
            query.validate().map_err(anyhow::Error::msg)?;
            if let Some(name) = &query.name {
                sql.push_str(" AND (display_name=$plugin_read_name OR ($plugin_read_name='' AND display_name=NONE))");
                bindings.push(("plugin_read_name".into(), json!(name)));
            }
            if let Some(property) = &query.property {
                if !admission.allows_property(&property.key) {
                    return Err(refuse(
                        PluginRefusal::CapabilityDenied,
                        "plugin property capability denied",
                    ));
                }
            }
            sql.push_str(" ORDER BY node_id LIMIT $plugin_read_limit");
            bindings.push(("plugin_read_limit".into(), json!(PLUGIN_QUERY_CANDIDATES + 1)));
        }
        PluginRead::PetalScope => unreachable!(),
    }
    let result_slot = guards.len() + 1;
    guards.push(sql);
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        guards.join("; ")
    ));
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut response = query.await?.check()?;
    let rows: Vec<Value> = response.take(result_slot)?;
    anyhow::ensure!(!is_single || rows.len() <= 1, "ambiguous node identity");
    anyhow::ensure!(
        is_single || rows.len() <= PLUGIN_QUERY_CANDIDATES,
        "plugin query candidate bound exceeded; narrow the name or petal"
    );
    anyhow::ensure!(
        serde_json::to_vec(&rows)?.len() <= 2 * 1024 * 1024,
        "plugin query candidates exceed 2 MiB"
    );
    let mut nodes: Vec<_> = rows
        .into_iter()
        .map(|row| snapshot_for_plugin(row, admission))
        .collect::<anyhow::Result<_>>()?;
    if let PluginRead::Nodes(query) = &request {
        nodes.retain(|node| query.matches(node));
        nodes.truncate(query.limit);
    }
    anyhow::ensure!(
        serde_json::to_vec(&nodes)?.len() <= MAX_HOST_BYTES,
        "plugin read exceeds 65536 bytes"
    );
    authorize_as(
        db,
        admission,
        local_did,
        fe_policy::RoleLevel::Viewer,
        admission.allows(fe_sdk::CAP_STORAGE_READ),
        false,
    )
    .await?;
    admission
        .require_active()
        .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
    Ok(if is_single {
        PluginReply::Node(nodes.into_iter().next())
    } else {
        PluginReply::Nodes(nodes)
    })
}

struct StagedNode {
    state: Value,
    previous: Option<Value>,
    created: bool,
    deleted: bool,
    transformed: bool,
    properties_changed: bool,
}

/// One node a committed batch wrote.
#[derive(Debug)]
pub(crate) struct CommittedNode {
    pub node_id: String,
    pub state: Value,
    pub created: bool,
    pub deleted: bool,
    /// Its property bag changed, so GUI property caches reload it.
    pub properties_changed: bool,
}

/// Post-commit effects consumed by the ordinary DB dispatcher bridges.
#[derive(Debug)]
pub(crate) struct BatchOutcome {
    pub receipt: TransactionReceipt,
    pub scope: String,
    pub nodes: Vec<CommittedNode>,
    pub changes: Vec<SceneChange>,
}

fn empty_outcome(scope: String) -> BatchOutcome {
    BatchOutcome {
        receipt: TransactionReceipt {
            op_count: 0,
            created_node_ids: vec![],
            reading_count: 0,
        },
        scope,
        nodes: vec![],
        changes: vec![],
    }
}

/// The manifest write kind an op stages, with its wire name.
fn write_kind(op: &PendingOp) -> (SceneWrite, &'static str) {
    match op {
        PendingOp::CreateNode { .. } => (SceneWrite::CreateNode, "create_node"),
        PendingOp::SetProperty { .. } => (SceneWrite::SetProperty, "set_property"),
        PendingOp::DeleteNode { .. } => (SceneWrite::DeleteNode, "delete_node"),
        PendingOp::SetTransform { .. } => (SceneWrite::SetTransform, "set_transform"),
    }
}

/// Resolve a transform patch over the staged state into the journal's Euler-XYZ `f32` transform.
fn resolve_transform(
    state: &Value,
    position: Option<[f64; 3]>,
    rotation: Option<[f64; 4]>,
    scale: Option<[f64; 3]>,
) -> anyhow::Result<([f32; 3], [f32; 3], [f32; 3])> {
    fn narrow(values: &[f64]) -> anyhow::Result<[f32; 3]> {
        let [x, y, z] = values else {
            anyhow::bail!("plugin transform needs three components");
        };
        let narrowed = [*x as f32, *y as f32, *z as f32];
        anyhow::ensure!(
            narrowed.iter().all(|value| value.is_finite()),
            "plugin transform exceeds render precision"
        );
        Ok(narrowed)
    }
    fn euler(rotation: Quat) -> anyhow::Result<[f32; 3]> {
        anyhow::ensure!(
            rotation.is_finite() && rotation.length_squared() > f32::EPSILON,
            "plugin rotation must be a finite nonzero quaternion"
        );
        let (x, y, z) = rotation.normalize().to_euler(EulerRot::XYZ);
        Ok([x, y, z])
    }
    let stored = |key: &str| -> anyhow::Result<Vec<f64>> {
        state[key]
            .as_array()
            .and_then(|values| values.iter().map(Value::as_f64).collect::<Option<Vec<f64>>>())
            .ok_or_else(|| anyhow::anyhow!("stored node {key} is unavailable"))
    };
    let position = match position {
        Some(position) => narrow(&position)?,
        None => narrow(&stored("position")?)?,
    };
    let scale = match scale {
        Some(scale) => narrow(&scale)?,
        None => narrow(&stored("scale")?)?,
    };
    let rotation = match rotation {
        Some(quaternion) => euler(Quat::from_array(quaternion.map(|value| value as f32)))?,
        None => {
            let current = stored("rotation")?;
            if current.len() == 3 {
                narrow(&current)?
            } else {
                // Legacy initial rows hold a quaternion; full transforms store Euler XYZ.
                let quaternion = fe_runtime::node_history::stored_rotation_quaternion(&current)
                    .ok_or_else(|| anyhow::anyhow!("stored node rotation is unavailable"))?;
                euler(Quat::from_array(quaternion))?
            }
        }
    };
    Ok((position, rotation, scale))
}

pub(crate) async fn commit(
    db: &Db,
    admission: &PluginAdmission,
    ops: &[PendingOp],
    local_did: &str,
) -> anyhow::Result<BatchOutcome> {
    fe_sdk::plugin_host::validate_transaction(ops)
        .map_err(|error| refuse(PluginRefusal::InvalidInput, error))?;
    for op in ops {
        let (kind, name) = write_kind(op);
        if !admission.allows_write(kind) {
            return Err(refuse(
                PluginRefusal::CapabilityDenied,
                format!("plugin {name} write denied"),
            ));
        }
    }
    let (scope, permission) = authorize_as(
        db,
        admission,
        local_did,
        fe_policy::RoleLevel::Editor,
        admission.allows(fe_sdk::CAP_STORAGE_WRITE),
        false,
    )
    .await?;
    if ops.is_empty() {
        admission
            .require_active()
            .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
        permission
            .commit(db, &[], vec![])
            .await
            .map_err(|error| refuse(PluginRefusal::Revoked, format!("{error:#}")))?;
        return Ok(empty_outcome(scope));
    }
    let (mut guards, mut bindings) = permission.into_parts();
    let mut staged: BTreeMap<String, StagedNode> = BTreeMap::new();
    let mut rules: HashMap<String, Vec<Value>> = HashMap::new();
    let mut facts: Vec<(String, String, i64, Value)> = Vec::new();
    let mut created_node_ids = Vec::new();
    let mut changes = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let node_id = match op {
            PendingOp::CreateNode { petal_id, name } => {
                anyhow::ensure!(
                    petal_id == admission.petal_id(),
                    "plugin creation targets another petal"
                );
                let id = ulid::Ulid::new().to_string();
                let state = json!({"node_id":id,"petal_id":petal_id,"display_name":name,"position":[0.0,0.0,0.0],"elevation":0.0,
                    "rotation":[0.0,0.0,0.0],"scale":[1.0,1.0,1.0],"properties":{},"interactive":false,"edit_seq":0,"created_at":chrono::Utc::now().to_rfc3339()});
                let payload = json!({"journal_version":1,"scope":scope,"state":state,"after_state":state,"coverage_reason":"plugin_object_created","schema_versions":{},"plugin_id":admission.plugin_id()});
                facts.push((id.clone(), "baseline".into(), 1, payload.clone()));
                staged.insert(
                    id.clone(),
                    StagedNode {
                        state,
                        previous: Some(json!({"row_version":1,"payload":payload})),
                        created: true,
                        deleted: false,
                        transformed: false,
                        properties_changed: false,
                    },
                );
                created_node_ids.push(id);
                continue;
            }
            PendingOp::SetProperty { node_id, .. }
            | PendingOp::DeleteNode { node_id }
            | PendingOp::SetTransform { node_id, .. } => node_id.clone(),
        };
        if !staged.contains_key(&node_id) {
            let mut response = db
                .query("SELECT * OMIT id FROM node WHERE node_id=$id")
                .bind(("id", node_id.clone()))
                .await?
                .check()?;
            let rows: Vec<Value> = response.take(0)?;
            anyhow::ensure!(
                rows.len() == 1
                    && rows[0]["petal_id"] == admission.petal_id()
                    && rows[0].get("tombstone").is_none_or(Value::is_null),
                "node not found or not permitted"
            );
            let mut raw = rows[0].clone();
            let state = node_history::normalize_snapshot(raw.clone())?;
            raw.as_object_mut().unwrap().remove("position");
            let prefix = format!("plugin_node_{index}");
            guards.push(format!("IF (SELECT * OMIT id, position FROM node WHERE node_id=${prefix}_id) != ${prefix}_before {{ THROW 'plugin node changed'; }}"));
            guards.push(format!("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=${prefix}_id AND position=<geometry<point>> [${prefix}_x,${prefix}_z])) != 1 {{ THROW 'plugin node position changed'; }}"));
            let previous = node_history::latest(db, &node_id).await?;
            let previous_rows: Vec<Value> = previous.iter().cloned().collect();
            guards.push(format!("IF (SELECT * OMIT id FROM node_log WHERE node_id=${prefix}_id ORDER BY row_version DESC LIMIT 1) != ${prefix}_history {{ THROW 'plugin history changed'; }}"));
            bindings.extend([
                (format!("{prefix}_id"), json!(node_id)),
                // A SELECT is an array: bind `[raw]` like every sibling fence.
                (format!("{prefix}_before"), json!([raw])),
                (format!("{prefix}_x"), state["position"][0].clone()),
                (format!("{prefix}_z"), state["position"][2].clone()),
                (format!("{prefix}_history"), json!(previous_rows)),
            ]);
            staged.insert(
                node_id.clone(),
                StagedNode {
                    state,
                    previous,
                    created: false,
                    deleted: false,
                    transformed: false,
                    properties_changed: false,
                },
            );
        }
        let node = staged.get_mut(&node_id).unwrap();
        anyhow::ensure!(!node.deleted, "plugin batch accesses a node after deletion");
        let (operation, data, schema_versions) = match op {
            PendingOp::SetProperty { key, value, .. } => {
                if !admission.allows_property(key) {
                    return Err(refuse(
                        PluginRefusal::CapabilityDenied,
                        "plugin property capability denied",
                    ));
                }
                if !rules.contains_key(key) {
                    let snapshot = field_def::capture_property_rules(db, &scope, key).await?;
                    let prefix = format!("plugin_rule_{index}");
                    let statement = field_def::PROPERTY_RULES_SQL
                        .replace("$key", &format!("${prefix}_key"))
                        .replace("$scopes", &format!("${prefix}_scopes"));
                    guards.push(format!("IF ({statement}) != ${prefix}_snapshot {{ THROW 'plugin schema changed'; }}"));
                    bindings.extend([
                        (format!("{prefix}_key"), json!(key)),
                        (
                            format!("{prefix}_scopes"),
                            json!(field_def::property_rule_scopes(&scope)),
                        ),
                        (format!("{prefix}_snapshot"), json!(snapshot)),
                    ]);
                    rules.insert(key.clone(), snapshot);
                }
                let versions =
                    field_def::validate_captured_property_rules(&scope, key, value, &rules[key])?;
                changes.push(SceneChange::PropertyChanged {
                    node_id: node_id.clone(),
                    petal_id: admission.petal_id().into(),
                    key: key.clone(),
                    value: value.clone(),
                });
                node.properties_changed = true;
                ("property_set", json!({"key":key,"value":value}), versions)
            }
            PendingOp::SetTransform {
                position,
                rotation,
                scale,
                ..
            } => {
                let properties = &node.state["properties"];
                anyhow::ensure!(
                    ["gpx_points", "owning_path_id", fe_sdk::PATH_ASSET_PROPERTY_KEY]
                        .iter()
                        .all(|key| properties.get(*key).is_none())
                        && properties.get("node_kind").and_then(Value::as_str) != Some("stamp"),
                    "plugin transforms refuse path and stamp geometry"
                );
                let (position, rotation, scale) =
                    resolve_transform(&node.state, *position, *rotation, *scale)?;
                changes.push(SceneChange::NodeTransform {
                    node_id: node_id.clone(),
                    petal_id: admission.petal_id().into(),
                    position,
                    rotation,
                    scale,
                });
                node.transformed = true;
                (
                    "full_transform",
                    json!({"position":position,"rotation":rotation,"scale":scale}),
                    json!({}),
                )
            }
            PendingOp::DeleteNode { .. } => {
                anyhow::ensure!(
                    [
                        "gpx_track_id",
                        "owning_path_id",
                        fe_sdk::PATH_ASSET_PROPERTY_KEY
                    ]
                    .iter()
                    .all(|key| node.state["properties"].get(*key).is_none()),
                    "plugin v1 refuses path/stamp lifecycle deletion"
                );
                let mut children=db.query("SELECT VALUE node_id FROM node WHERE (properties.parent_id=$id OR properties.gpx_track_id=$id OR properties.owning_path_id=$id) AND tombstone=NONE LIMIT 1").bind(("id",node_id.clone())).await?.check()?;
                anyhow::ensure!(
                    children.take::<Vec<Value>>(0)?.is_empty(),
                    "plugin v1 refuses deletion with dependent children"
                );
                let name = format!("plugin_delete_{index}");
                guards.push(format!("IF array::len((SELECT VALUE node_id FROM node WHERE (properties.parent_id=${name} OR properties.gpx_track_id=${name} OR properties.owning_path_id=${name}) AND tombstone=NONE LIMIT 1)) != 0 {{ THROW 'plugin deletion gained dependent children'; }}"));
                bindings.push((name, json!(node_id)));
                node.deleted = true;
                changes.push(SceneChange::NodeRemoved {
                    node_id: node_id.clone(),
                    petal_id: admission.petal_id().into(),
                });
                (
                    "plugin_tombstone",
                    json!({"marker":{"hlc":u64::MAX,"source_did":local_did,"tombstoned_at":"9999-12-31T23:59:59.999999999Z"}}),
                    json!({}),
                )
            }
            PendingOp::CreateNode { .. } => unreachable!(),
        };
        let change = node_history::prepare_change(
            node.state.clone(),
            node.previous.as_ref(),
            &scope,
            operation,
            data,
            schema_versions,
        )?;
        node.state = change.after;
        for (operation, version, mut payload) in change.facts {
            payload["plugin_id"] = json!(admission.plugin_id());
            node.previous = Some(json!({"row_version":version,"payload":payload}));
            facts.push((node_id.clone(), operation, version, payload));
        }
    }
    for (deleted_id, deleted) in &staged {
        if !deleted.deleted {
            continue;
        }
        anyhow::ensure!(
            staged.iter().all(|(id, node)| id == deleted_id
                || node.deleted
                || ["parent_id", "gpx_track_id", "owning_path_id"]
                    .iter()
                    .all(
                        |key| node.state["properties"].get(*key).and_then(Value::as_str)
                            != Some(deleted_id.as_str())
                    )),
            "plugin batch introduces a dependent child of a deleted node"
        );
    }
    let expanded_bytes = serde_json::to_vec(&facts)?.len()
        + staged
            .values()
            .map(|node| serde_json::to_vec(&node.state).map(|bytes| bytes.len()))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .sum::<usize>();
    anyhow::ensure!(
        expanded_bytes <= 2 * 1024 * 1024,
        "expanded plugin batch exceeds 2 MiB"
    );
    let mut intention = json!({"plugin_id":admission.plugin_id(),"petal_id":admission.petal_id(),"scope":scope,"source_did":local_did,"ops":ops});
    // Local attribution only; replicated provenance stamping waits for OD-E1 (R24).
    if let Some(package) = admission.package() {
        intention["package"] = json!({"package_id":package.package_id(),"content_hash":package.content_hash()});
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(admission.petal_id().into()),
        op_type: OpType::PluginTransaction,
        payload: intention,
        sig: "00".repeat(64),
    };
    admission
        .require_active()
        .map_err(|error| refuse(PluginRefusal::Revoked, error))?;
    crate::op_log::commit_operation_atomic_with(db,entry,|hlc| {
        for (index,(id,node)) in staged.iter_mut().enumerate() {
            let name=format!("plugin_write_{index}");
            if node.deleted {node.state["tombstone"]=super::crud::tombstone_marker(hlc,local_did);}
            if node.created {
                guards.push(format!("CREATE node CONTENT object::extend(${name}_state,{{position:<geometry<point>> [0.0,0.0]}})"));
                bindings.push((format!("{name}_state"),node.state.clone()));
            } else {
                let transform=if node.transformed {format!(", position=<geometry<point>> [${name}_x,${name}_z], elevation=${name}_y, rotation=${name}_rotation, scale=${name}_scale, edit_seq=${name}_seq")} else {String::new()};
                let tombstone=if node.deleted {format!(", tombstone=${name}_tombstone")} else {String::new()};
                guards.push(format!("UPDATE node SET properties=${name}_properties{transform}{tombstone} WHERE node_id=${name}_id"));
                bindings.extend([(format!("{name}_id"),json!(id)),(format!("{name}_properties"),node.state["properties"].clone())]);
                if node.transformed {
                    for (suffix,value) in [("x",&node.state["position"][0]),("y",&node.state["position"][1]),("z",&node.state["position"][2]),
                        ("rotation",&node.state["rotation"]),("scale",&node.state["scale"]),("seq",&node.state["edit_seq"])] {
                        bindings.push((format!("{name}_{suffix}"),value.clone()));
                    }
                }
                if node.deleted {bindings.push((format!("{name}_tombstone"),node.state["tombstone"].clone()));}
            }
        }
        for (index,(node_id,operation,version,payload)) in facts.iter_mut().enumerate() {
            if operation=="plugin_tombstone" {let marker=staged.get(node_id.as_str()).unwrap().state["tombstone"].clone();payload["after_state"]["tombstone"]=marker.clone();payload["data"]["marker"]=marker;}
            let name=format!("plugin_fact_{index}");
            guards.push(format!("CREATE node_log CONTENT ${name}"));
            bindings.push((name,json!({"log_id":ulid::Ulid::new().to_string(),"node_id":node_id,"hlc_timestamp":hlc as i64,"op":operation,"source_did":local_did,
                "payload":payload,"row_version":version,"created_at":chrono::Utc::now().to_rfc3339()})));
        }
        (guards,bindings)
    }).await.map_err(|error| refuse(PluginRefusal::Backend, format!("{error:#}")))?;
    Ok(BatchOutcome {
        receipt: TransactionReceipt {
            op_count: ops.len(),
            created_node_ids,
            reading_count: 0,
        },
        scope,
        nodes: staged
            .into_iter()
            .map(|(node_id, node)| CommittedNode {
                node_id,
                state: node.state,
                created: node.created,
                deleted: node.deleted,
                properties_changed: node.properties_changed,
            })
            .collect(),
        changes,
    })
}

/// Check staged readings as the readings door does (declared metric, RFC-3339 or epoch-ms time,
/// anchors of the bound petal) before any effect, so one bad reading refuses the whole flush (R6).
async fn door_readings(
    db: &Db,
    admission: &PluginAdmission,
    readings: &[StagedReading],
) -> anyhow::Result<Vec<IotReadingInput>> {
    use super::iot_reading::IotIngestError;
    if readings.is_empty() {
        return Ok(Vec::new());
    }
    let grant = admission.readings().ok_or_else(|| {
        refuse(
            PluginRefusal::CapabilityDenied,
            "plugin readings capability denied",
        )
    })?;
    let invalid_time =
        |raw: String| refuse(PluginRefusal::InvalidInput, IotIngestError::InvalidTimestamp(raw).to_string());
    let mut anchors = BTreeSet::new();
    let mut inputs = Vec::with_capacity(readings.len());
    for reading in readings {
        if !grant.allows_metric(&reading.metric) {
            return Err(refuse(
                PluginRefusal::CapabilityDenied,
                format!("plugin metric '{}' is not declared", reading.metric),
            ));
        }
        let recorded_at = match &reading.recorded_at {
            None => None,
            Some(StagedTimestamp::Rfc3339(raw)) => {
                chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| invalid_time(raw.clone()))?;
                Some(raw.clone())
            }
            Some(StagedTimestamp::EpochMs(millis)) => Some(
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*millis)
                    .ok_or_else(|| invalid_time(millis.to_string()))?
                    .to_rfc3339(),
            ),
        };
        anchors.insert(reading.node_id.clone());
        inputs.push(IotReadingInput {
            node_id: reading.node_id.clone(),
            metric: reading.metric.clone(),
            value: reading.value,
            units: reading.units.clone(),
            recorded_at,
            payload_id: None,
        });
    }
    // The door's anchor rule: an existing node row of the bound petal.
    let anchor_ids: Vec<String> = anchors.into_iter().collect();
    let mut response = db
        .query("SELECT VALUE node_id FROM node WHERE petal_id=$plugin_anchor_petal AND node_id IN $plugin_anchor_ids")
        .bind(("plugin_anchor_petal", admission.petal_id().to_string()))
        .bind(("plugin_anchor_ids", anchor_ids.clone()))
        .await?
        .check()?;
    let known: Vec<String> = response.take(0)?;
    if let Some(unknown) = anchor_ids.iter().find(|id| !known.contains(*id)) {
        return Err(refuse(
            PluginRefusal::InvalidInput,
            IotIngestError::UnknownAnchor(unknown.clone()).to_string(),
        ));
    }
    Ok(inputs)
}

/// Authorize a readings-only flush: the readings grant, the Editor floor and the pinned ancestry.
async fn authorize_readings(
    db: &Db,
    admission: &PluginAdmission,
    local_did: &str,
) -> anyhow::Result<BatchOutcome> {
    let (scope, permission) = authorize_as(
        db,
        admission,
        local_did,
        fe_policy::RoleLevel::Editor,
        admission.readings().is_some(),
        false,
    )
    .await?;
    permission
        .commit(db, &[], vec![])
        .await
        .map_err(|error| refuse(PluginRefusal::Revoked, format!("{error:#}")))?;
    Ok(empty_outcome(scope))
}

/// Everything a flush publishes through; the DB thread's PluginCommit arm supplies it.
pub(crate) struct FlushSinks<'a> {
    pub effects: EffectSinks<'a>,
    pub blobs: &'a crate::BlobStoreHandle,
    pub replication: Option<&'a crate::ReplicationSender>,
    /// Transform-only nodes coalesce here like `UpdateNodeTransform` (QA-0161).
    pub deferred: &'a mut crate::command_lane::DeferredPublications,
}

/// One tick's write window (R5, R6): the readings pass the door's checks, the scene batch commits
/// atomically, then the readings land through the door as the admitted source class.
pub(crate) async fn flush(
    db: &Db,
    admission: &PluginAdmission,
    ops: &[PendingOp],
    readings: &[StagedReading],
    local_did: &str,
    mut sinks: FlushSinks<'_>,
) -> Result<TransactionReceipt, PluginRefusal> {
    admission.require_active().map_err(PluginRefusal::Revoked)?;
    fe_sdk::plugin_host::validate_commit(ops, readings).map_err(PluginRefusal::InvalidInput)?;
    let door = door_readings(db, admission, readings)
        .await
        .map_err(refusal)?;
    let committed = if ops.is_empty() && !door.is_empty() {
        authorize_readings(db, admission, local_did).await
    } else {
        commit(db, admission, ops, local_did).await
    };
    let outcome = committed.map_err(refusal)?;
    // Past the commit admission boundary: everything below runs even if the lease is revoked now.
    for node in &outcome.nodes {
        if node.created || node.deleted || node.properties_changed {
            super::scene_publication::publish_after_commit(
                db,
                sinks.blobs,
                sinks.replication,
                local_did,
                &node.node_id,
            )
            .await;
        } else {
            sinks.deferred.defer(node.node_id.clone());
        }
    }
    publish(&outcome, sinks.effects);
    let landed = match admission.readings() {
        Some(grant) if !door.is_empty() => {
            let verse_id = crate::parse_scope(&outcome.scope)
                .map_err(|error| PluginRefusal::UnknownOutcome(format!("flush scope unusable: {error}")))?
                .verse_id;
            super::iot_reading::insert_readings_with_replication(
                db,
                admission.petal_id(),
                Some(verse_id.as_str()),
                local_did,
                grant.source_class(),
                &door,
                Some(sinks.blobs),
                sinks.replication,
            )
            .await
            .map_err(|error| {
                // The door validates before it writes; only a scene batch already committed makes this partial.
                if ops.is_empty() && error.validation_rejection().is_some() {
                    PluginRefusal::InvalidInput(error.to_string())
                } else {
                    PluginRefusal::UnknownOutcome(format!(
                        "the flush was admitted but its readings did not all land: {error}"
                    ))
                }
            })?
        }
        _ => 0,
    };
    let mut receipt = outcome.receipt;
    receipt.reading_count = landed;
    Ok(receipt)
}

pub(crate) struct EffectSinks<'a> {
    pub results: &'a crossbeam::channel::Sender<fe_runtime::messages::DbResult>,
    pub scene: Option<&'a super::replicated_row::SceneChangeSender>,
    pub lifecycle: &'a Option<crossbeam::channel::Sender<fe_runtime::messages::LifecycleEvent>>,
}

/// Publish only the state that survived the atomic commit.
fn publish(outcome: &BatchOutcome, sinks: EffectSinks<'_>) {
    use fe_runtime::messages::{DbResult, LifecycleEvent};
    let Ok(parts) = crate::parse_scope(&outcome.scope) else {
        tracing::error!("committed plugin scope invalid");
        return;
    };
    let Some(petal_id) = parts.petal_id else {
        tracing::error!("committed plugin petal missing");
        return;
    };
    for node in &outcome.nodes {
        let node_id = &node.node_id;
        if node.deleted {
            super::send_result(
                sinks.results,
                DbResult::PluginNodeRemoved {
                    node_id: node_id.clone(),
                    petal_id: petal_id.clone(),
                },
            );
            if let Some(address) = crate::lifecycle_uri(&outcome.scope, node_id) {
                crate::emit_lifecycle(
                    sinks.lifecycle,
                    LifecycleEvent::NodeDeleted {
                        address,
                        node_id: node_id.clone(),
                    },
                );
            }
        } else if node.created {
            let materialization = serde_json::from_value(node.state.clone())
                .map_err(anyhow::Error::from)
                .and_then(|state| {
                    node_history::materialization(
                        &fe_runtime::node_history::NodeMutationReceipt {
                            node_id: node_id.clone(),
                            history_version: 1,
                            state,
                            persist: fe_runtime::node_history::PersistStatus::Committed,
                        },
                        None,
                    )
                });
            match materialization {
                Ok(node) => {
                    super::replicated_row::emit_scene_change(
                        sinks.scene,
                        SceneChange::NodeAdded {
                            node: node.node.clone(),
                        },
                    );
                    super::send_result(
                        sinks.results,
                        DbResult::NodeMaterialized {
                            node,
                            correlation_id: format!("plugin-birth-{}", ulid::Ulid::new()),
                        },
                    );
                }
                Err(error) => {
                    tracing::error!(%error,node_id,"committed plugin birth could not materialize")
                }
            }
            if let Some(address) = crate::lifecycle_uri(&outcome.scope, node_id) {
                crate::emit_lifecycle(
                    sinks.lifecycle,
                    LifecycleEvent::NodeCreated {
                        address,
                        node_id: node_id.clone(),
                    },
                );
            }
        } else if node.properties_changed {
            super::send_result(
                sinks.results,
                DbResult::NodePropertiesLoaded {
                    node_id: node_id.clone(),
                    properties: node.state["properties"].clone(),
                    correlation_id: Some(format!("plugin-properties-{}", ulid::Ulid::new())),
                },
            );
        }
    }
    // Transforms reach the GUI and entity store through the scene feed, as UpdateNodeTransform does.
    for change in &outcome.changes {
        super::replicated_row::emit_scene_change(sinks.scene, change.clone());
    }
}
