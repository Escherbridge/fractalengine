//! Typed plugin reads and atomic batches; see src/AGENTS.md §plugin-host.

use super::{entity, field_def, node_history};
use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::plugin_host::{PluginAdmission, PluginRead, PluginReply};
use fe_sdk::plugin_host::{PendingOp, TransactionReceipt, MAX_HOST_BYTES};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const PLUGIN_QUERY_CANDIDATES: usize = 1024;

async fn authorize_as(
    db: &Db,
    admission: &PluginAdmission,
    local_did: &str,
    floor: fe_policy::RoleLevel,
    capability: &str,
    handshake: bool,
) -> anyhow::Result<(String, entity::ParentMutation)> {
    admission.require_active().map_err(anyhow::Error::msg)?;
    anyhow::ensure!(admission.allows(capability), "plugin capability denied");
    let scope =
        super::preconditions::require_petal_scope(db, admission.petal_id(), "PluginHost").await?;
    let parts = crate::parse_scope(&scope)?;
    anyhow::ensure!(
        parts.petal_id.as_deref() == Some(admission.petal_id()),
        "plugin scope mismatch"
    );
    anyhow::ensure!(
        admission.allows_scope(&crate::build_scope(&parts.verse_id, None, None), &scope),
        "plugin scope denied"
    );
    anyhow::ensure!(
        admission.bound_scope() == Some(scope.as_str())
            || (handshake && admission.bound_scope().is_none()),
        "plugin admitted ancestry changed"
    );
    let permission = entity::prepare_scope_permission(db, &scope, local_did, floor).await?;
    admission.require_active().map_err(anyhow::Error::msg)?;
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
    let (floor, capability) = if handshake && !admission.allows(fe_sdk::CAP_STORAGE_READ) {
        (fe_policy::RoleLevel::Editor, fe_sdk::CAP_STORAGE_WRITE)
    } else {
        (fe_policy::RoleLevel::Viewer, fe_sdk::CAP_STORAGE_READ)
    };
    let (scope, permission) =
        authorize_as(db, admission, local_did, floor, capability, handshake).await?;
    if matches!(request, PluginRead::PetalScope) {
        permission.commit(db, &[], vec![]).await?;
        admission.require_active().map_err(anyhow::Error::msg)?;
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
            anyhow::ensure!(
                admission.allows(fe_sdk::CAP_QUERY_SELECT),
                "plugin query capability denied"
            );
            query.validate().map_err(anyhow::Error::msg)?;
            if let Some(name) = &query.name {
                sql.push_str(" AND (display_name=$plugin_read_name OR ($plugin_read_name='' AND display_name=NONE))");
                bindings.push(("plugin_read_name".into(), json!(name)));
            }
            if let Some(property) = &query.property {
                anyhow::ensure!(
                    admission.allows_property(&property.key),
                    "plugin property capability denied"
                );
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
        fe_sdk::CAP_STORAGE_READ,
        false,
    )
    .await?;
    admission.require_active().map_err(anyhow::Error::msg)?;
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
}

/// Post-commit effects consumed by the ordinary DB dispatcher bridges.
#[derive(Debug)]
pub(crate) struct BatchOutcome {
    pub receipt: TransactionReceipt,
    pub scope: String,
    pub nodes: Vec<(String, Value, bool, bool)>,
    pub changes: Vec<fe_runtime::messages::SceneChange>,
}

pub(crate) async fn commit(
    db: &Db,
    admission: &PluginAdmission,
    ops: &[PendingOp],
    local_did: &str,
) -> anyhow::Result<BatchOutcome> {
    fe_sdk::plugin_host::validate_transaction(ops).map_err(anyhow::Error::msg)?;
    let (scope, permission) = authorize_as(
        db,
        admission,
        local_did,
        fe_policy::RoleLevel::Editor,
        fe_sdk::CAP_STORAGE_WRITE,
        false,
    )
    .await?;
    if ops.is_empty() {
        admission.require_active().map_err(anyhow::Error::msg)?;
        permission.commit(db, &[], vec![]).await?;
        return Ok(BatchOutcome {
            receipt: TransactionReceipt {
                op_count: 0,
                created_node_ids: vec![],
            },
            scope,
            nodes: vec![],
            changes: vec![],
        });
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
                    },
                );
                created_node_ids.push(id);
                continue;
            }
            PendingOp::SetProperty { node_id, .. } | PendingOp::DeleteNode { node_id } => {
                node_id.clone()
            }
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
                (format!("{prefix}_before"), raw),
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
                },
            );
        }
        let node = staged.get_mut(&node_id).unwrap();
        anyhow::ensure!(!node.deleted, "plugin batch accesses a node after deletion");
        let (operation, data, schema_versions) = match op {
            PendingOp::SetProperty { key, value, .. } => {
                anyhow::ensure!(
                    admission.allows_property(key),
                    "plugin property capability denied"
                );
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
                changes.push(fe_runtime::messages::SceneChange::PropertyChanged {
                    node_id: node_id.clone(),
                    petal_id: admission.petal_id().into(),
                    key: key.clone(),
                    value: value.clone(),
                });
                ("property_set", json!({"key":key,"value":value}), versions)
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
                changes.push(fe_runtime::messages::SceneChange::NodeRemoved {
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
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(admission.petal_id().into()),
        op_type: OpType::PluginTransaction,
        payload: json!({"plugin_id":admission.plugin_id(),"petal_id":admission.petal_id(),"scope":scope,"source_did":local_did,"ops":ops}),
        sig: "00".repeat(64),
    };
    admission.require_active().map_err(anyhow::Error::msg)?;
    crate::op_log::commit_operation_atomic_with(db,entry,|hlc| {
        for (index,(id,node)) in staged.iter_mut().enumerate() {
            let name=format!("plugin_write_{index}");
            if node.deleted {node.state["tombstone"]=super::crud::tombstone_marker(hlc,local_did);}
            if node.created {
                guards.push(format!("CREATE node CONTENT object::extend(${name}_state,{{position:<geometry<point>> [0.0,0.0]}})"));
                bindings.push((format!("{name}_state"),node.state.clone()));
            } else {
                guards.push(format!("UPDATE node SET properties=${name}_properties{} WHERE node_id=${name}_id",if node.deleted {format!(", tombstone=${name}_tombstone")}else {String::new()}));
                bindings.extend([(format!("{name}_id"),json!(id)),(format!("{name}_properties"),node.state["properties"].clone())]);
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
    }).await?;
    Ok(BatchOutcome {
        receipt: TransactionReceipt {
            op_count: ops.len(),
            created_node_ids,
        },
        scope,
        nodes: staged
            .into_iter()
            .map(|(id, node)| (id, node.state, node.created, node.deleted))
            .collect(),
        changes,
    })
}

pub(crate) struct EffectSinks<'a> {
    pub results: &'a crossbeam::channel::Sender<fe_runtime::messages::DbResult>,
    pub scene: Option<&'a super::replicated_row::SceneChangeSender>,
    pub lifecycle: &'a Option<crossbeam::channel::Sender<fe_runtime::messages::LifecycleEvent>>,
    pub replication: Option<&'a crate::ReplicationSender>,
    pub blobs: &'a crate::BlobStoreHandle,
}

/// Publish only the state that survived the atomic commit.
pub(crate) fn publish(outcome: &BatchOutcome, sinks: EffectSinks<'_>) {
    use fe_runtime::messages::{DbResult, LifecycleEvent, SceneChange};
    let Ok(parts) = crate::parse_scope(&outcome.scope) else {
        tracing::error!("committed plugin scope invalid");
        return;
    };
    let Some(petal_id) = parts.petal_id else {
        tracing::error!("committed plugin petal missing");
        return;
    };
    for (node_id, state, created, deleted) in &outcome.nodes {
        let mut row = state.clone();
        row["position"] =
            json!({"type":"Point","coordinates":[state["position"][0],state["position"][2]]});
        match serde_json::to_vec(&row) {
            Ok(bytes) => crate::replicate_row_with_petal(
                sinks.replication,
                sinks.blobs,
                &parts.verse_id,
                "node",
                node_id,
                &bytes,
                Some(petal_id.clone()),
            ),
            Err(error) => {
                tracing::error!(%error,node_id,"committed plugin row could not replicate")
            }
        }
        if *deleted {
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
        } else if *created {
            let materialization = serde_json::from_value(state.clone())
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
        } else {
            super::send_result(
                sinks.results,
                DbResult::NodePropertiesLoaded {
                    node_id: node_id.clone(),
                    properties: state["properties"].clone(),
                    correlation_id: Some(format!("plugin-properties-{}", ulid::Ulid::new())),
                },
            );
        }
    }
    for change in &outcome.changes {
        super::replicated_row::emit_scene_change(sinks.scene, change.clone());
    }
}
