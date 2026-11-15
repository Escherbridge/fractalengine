//! Atomic ordinary node journal and bounded replay; see src/AGENTS.md §node-history.

use std::collections::BTreeMap;

use fe_runtime::node_history::{NodeAccessGrant, NodeAtomicSave};
use serde_json::{Value, json};

use crate::{op_log::commit_operation_atomic_with, repo::Db, types::OpLogEntry};

pub(crate) const JOURNAL_VERSION: u64 = 1;
const MAX_REPLAY_ROWS: usize = 10_000;
/// Serialized entry bytes after which a history page ends early (AGENTS.md §history-paging).
pub const HISTORY_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Journal rows read per round while a history page fills.
const HISTORY_FETCH_ROWS: usize = 8;
/// Operations whose stored `after_state` a replay may return (the reducer's vocabulary).
const REPLAYABLE_OPERATIONS: [&str; 4] = ["node_renamed", "property_set", "property_deleted", "full_transform"];
/// Payload keys a light fact keeps, with presence preserved.
const LIGHT_PAYLOAD_KEYS: [&str; 7] = [
    "journal_version",
    "scope",
    "stamp_batch_id",
    "atomic_save_id",
    "stamp_request_id",
    "atomic_save_request_id",
    "editor_request_id",
];
/// A journal fact without its stored states and change data: the identity, scope and receipt fields the
/// authorization and atomic-save checks read (AGENTS.md §history-replay).
const LIGHT_FACT_SELECT: &str = "SELECT node_id, log_id, row_version, op, source_did, \
    object::keys(payload) AS light_payload_keys, \
    payload.journal_version AS light_journal_version, payload.scope AS light_scope, \
    payload.stamp_batch_id AS light_stamp_batch_id, payload.atomic_save_id AS light_atomic_save_id, \
    payload.stamp_request_id AS light_stamp_request_id, payload.atomic_save_request_id AS light_atomic_save_request_id, \
    payload.editor_request_id AS light_editor_request_id, \
    payload.after_state.node_id AS light_after_node, payload.after_state.petal_id AS light_after_petal, \
    payload.state.node_id AS light_state_node, payload.state.petal_id AS light_state_petal FROM node_log";

/// Rebuild a light fact's `payload` from its projected columns, keeping only keys the stored payload has.
fn light_fact(mut row: Value) -> Value {
    let keys: Vec<String> = row["light_payload_keys"]
        .as_array()
        .map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let has = |key: &str| keys.iter().any(|present| present == key);
    let mut payload = serde_json::Map::new();
    for key in LIGHT_PAYLOAD_KEYS {
        if has(key) {
            payload.insert(key.to_owned(), row[format!("light_{key}")].clone());
        }
    }
    for (key, node, petal) in [("after_state", "light_after_node", "light_after_petal"), ("state", "light_state_node", "light_state_petal")] {
        if has(key) {
            payload.insert(key.to_owned(), json!({"node_id": row[node].clone(), "petal_id": row[petal].clone()}));
        }
    }
    if let Some(object) = row.as_object_mut() {
        object.retain(|column, _| !column.starts_with("light_"));
        object.insert("payload".to_owned(), Value::Object(payload));
    }
    row
}

fn require_grant(grant: &NodeAccessGrant, scope: &str) -> anyhow::Result<()> {
    let resource = crate::parse_scope(scope)?;
    if let NodeAccessGrant::Scoped(granted) = grant {
        let granted = crate::parse_scope(granted)?;
        if granted.verse_id != resource.verse_id
            || granted
                .fractal_id
                .as_ref()
                .is_some_and(|id| resource.fractal_id.as_ref() != Some(id))
            || granted
                .petal_id
                .as_ref()
                .is_some_and(|id| resource.petal_id.as_ref() != Some(id))
        {
            anyhow::bail!("not found or not permitted");
        }
    }
    Ok(())
}

pub(crate) async fn authorize_scope(
    db: &Db,
    grant: &NodeAccessGrant,
    local_did: &str,
    scope: &str,
    floor: fe_policy::RoleLevel,
) -> anyhow::Result<()> {
    require_grant(grant, scope)?;
    if matches!(grant, NodeAccessGrant::Local) {
        crate::require_node_role(db, local_did, scope, floor, "Object operation")
            .await
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub(crate) async fn authorized_node_scope(
    db: &Db,
    node_id: &str,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<String> {
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    authorize_scope(db, grant, local_did, &scope, fe_policy::RoleLevel::Editor).await?;
    Ok(scope)
}

pub(crate) fn scope_fence(
    scope: &str,
    node_id: Option<&str>,
) -> anyhow::Result<crate::GuardedStatements> {
    let parts = crate::parse_scope(scope)?;
    let fractal = parts
        .fractal_id
        .ok_or_else(|| anyhow::anyhow!("node scope has no fractal"))?;
    let petal = parts
        .petal_id
        .ok_or_else(|| anyhow::anyhow!("node scope has no petal"))?;
    let mut statements = vec![
        "IF array::len((SELECT VALUE verse_id FROM verse WHERE verse_id = $history_authorized_verse)) != 1 { THROW 'authorized ancestry changed'; }".into(),
        "IF array::len((SELECT VALUE fractal_id FROM fractal WHERE fractal_id = $history_authorized_fractal AND verse_id = $history_authorized_verse)) != 1 { THROW 'authorized ancestry changed'; }".into(),
        "IF array::len((SELECT VALUE petal_id FROM petal WHERE petal_id = $history_authorized_petal AND fractal_id = $history_authorized_fractal)) != 1 { THROW 'authorized ancestry changed'; }".into(),
    ];
    let mut bindings = vec![
        ("history_authorized_verse".into(), json!(parts.verse_id)),
        ("history_authorized_fractal".into(), json!(fractal)),
        ("history_authorized_petal".into(), json!(petal)),
    ];
    if let Some(id) = node_id {
        statements.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id = $history_authorized_node AND petal_id = $history_authorized_petal)) != 1 { THROW 'authorized ancestry changed'; }".into());
        bindings.push(("history_authorized_node".into(), json!(id)));
    }
    Ok((statements, bindings))
}

async fn fact_is_authorized(
    db: &Db,
    row: &Value,
    node_id: &str,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> bool {
    if row["node_id"].as_str() != Some(node_id)
        || row["payload"]["journal_version"] != JOURNAL_VERSION
    {
        return false;
    }
    let Some(scope) = row["payload"]["scope"].as_str() else {
        return false;
    };
    if authorize_scope(db, grant, local_did, scope, fe_policy::RoleLevel::Viewer)
        .await
        .is_err()
    {
        return false;
    }
    let Ok(parts) = crate::parse_scope(scope) else {
        return false;
    };
    let Some(petal_id) = parts.petal_id.as_deref() else {
        return false;
    };
    let matches = |state: &Value| {
        state["node_id"].as_str() == Some(node_id) && state["petal_id"].as_str() == Some(petal_id)
    };
    matches(&row["payload"]["after_state"])
        && (row["op"] != "baseline" || matches(&row["payload"]["state"]))
}

fn unavailable_fact(node_id: &str, version: i64) -> Value {
    json!({"log_id":format!("unavailable-{version}"),"node_id":node_id,"hlc_timestamp":0,"op":"unavailable_interval",
        "source_did":"","payload":{"availability":"unavailable_scope"},"row_version":version,"created_at":""})
}

#[derive(Clone)]
enum AtomicSaveRead {
    Untagged,
    Saved(NodeAtomicSave),
    Incomplete,
    UnavailableScope,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AtomicSaveTag {
    Stamp(String),
    Editor(String),
}

impl AtomicSaveTag {
    fn from_row(row: &Value) -> Result<Option<Self>, ()> {
        let payload = &row["payload"];
        let (value, stamp) = match (payload.get("stamp_batch_id"), payload.get("atomic_save_id")) {
            (None, None) => return Ok(None),
            (Some(value), None) => (value, true),
            (None, Some(value)) => (value, false),
            (Some(_), Some(_)) => return Err(()),
        };
        let id = value
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or(())?
            .to_owned();
        Ok(Some(if stamp {
            Self::Stamp(id)
        } else {
            Self::Editor(id)
        }))
    }

    fn id(&self) -> &str {
        match self {
            Self::Stamp(id) | Self::Editor(id) => id,
        }
    }
    fn field(&self) -> &'static str {
        match self {
            Self::Stamp(_) => "stamp_batch_id",
            Self::Editor(_) => "atomic_save_id",
        }
    }
    fn receipt_field(&self) -> &'static str {
        match self {
            Self::Stamp(_) => "stamp_request_id",
            Self::Editor(_) => "atomic_save_request_id",
        }
    }
    fn accepts_operation(&self, op: &Value) -> bool {
        match self {
            Self::Stamp(_) => op == "property_set",
            Self::Editor(_) => matches!(
                op.as_str(),
                Some("property_set" | "property_deleted" | "full_transform")
            ),
        }
    }
    fn receipt_matches(&self, payload: &Value, final_fact: bool) -> bool {
        [
            "stamp_request_id",
            "atomic_save_request_id",
            "editor_request_id",
        ]
        .into_iter()
        .all(|key| {
            if final_fact && (key == self.receipt_field() || key == "editor_request_id") {
                payload[key].as_str() == Some(self.id())
            } else {
                payload.get(key).is_none()
            }
        })
    }
}

/// Resolve explicit receipt tags across page boundaries; see AGENTS.md §atomic-replay.
async fn atomic_save(
    db: &Db,
    row: &Value,
    node_id: &str,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<AtomicSaveRead> {
    let tag = match AtomicSaveTag::from_row(row) {
        Ok(None) => return Ok(AtomicSaveRead::Untagged),
        Ok(Some(tag)) => tag,
        Err(()) => return Ok(AtomicSaveRead::Incomplete),
    };
    let mut response = db.query(format!("{LIGHT_FACT_SELECT} WHERE node_id = $id AND payload.{} = $batch ORDER BY row_version ASC LIMIT 201 TIMEOUT 5s",tag.field()))
        .bind(("id", node_id.to_string())).bind(("batch", tag.id().to_string())).await?.check()?;
    let rows: Vec<Value> = response.take::<Vec<Value>>(0)?.into_iter().map(light_fact).collect();
    if rows.is_empty() || rows.len() > 200 {
        return Ok(AtomicSaveRead::Incomplete);
    }
    for fact in &rows {
        if !fact_is_authorized(db, fact, node_id, grant, local_did).await {
            return Ok(AtomicSaveRead::UnavailableScope);
        }
    }
    let first_version = row_version(&rows[0])?;
    let final_index = rows.len() - 1;
    for (index, fact) in rows.iter().enumerate() {
        let payload = &fact["payload"];
        let final_fact = index == final_index;
        if Some(row_version(fact)?) != first_version.checked_add(index as i64)
            || !tag.accepts_operation(&fact["op"])
            || AtomicSaveTag::from_row(fact).ok().flatten().as_ref() != Some(&tag)
            || payload["scope"] != row["payload"]["scope"]
            || fact["source_did"] != row["source_did"]
            || !tag.receipt_matches(payload, final_fact)
        {
            return Ok(AtomicSaveRead::Incomplete);
        }
    }
    if !rows
        .iter()
        .any(|fact| fact["log_id"] == row["log_id"] && fact["row_version"] == row["row_version"])
    {
        return Ok(AtomicSaveRead::Incomplete);
    }
    Ok(AtomicSaveRead::Saved(NodeAtomicSave {
        batch_id: tag.id().to_string(),
        first_version,
        committed_version: row_version(&rows[final_index])?,
    }))
}

/// Stable prefix of the write-time refusal for a node over `MAX_NODE_BYTES`.
pub(crate) const NODE_TOO_LARGE: &str = "object snapshot exceeds the node size limit";

/// Refuse a node snapshot over `fe_sdk::property_schema::MAX_NODE_BYTES` (scene_publication/AGENTS.md §row-bound).
pub(crate) fn check_snapshot_size(value: &Value) -> anyhow::Result<()> {
    let limit = fe_sdk::property_schema::MAX_NODE_BYTES;
    if serde_json::to_vec(value)?.len() > limit {
        anyhow::bail!("{NODE_TOO_LARGE} of {limit} bytes");
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

pub(crate) fn normalize_snapshot(mut row: Value) -> anyhow::Result<Value> {
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

pub(crate) async fn latest(db: &Db, node_id: &str) -> anyhow::Result<Option<Value>> {
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn read_reply(
    db: &Db,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
    grant: &NodeAccessGrant,
    local_did: &str,
    correlation_id: Option<String>,
) -> fe_runtime::messages::DbResult {
    use fe_runtime::messages::DbResult;
    let result = async {
        let value = read_granted(
            db,
            node_id,
            after_version,
            limit,
            at_version,
            grant,
            local_did,
        )
        .await?;
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
    let rotation = fe_runtime::node_history::stored_rotation_quaternion(&state.rotation)
        .ok_or_else(|| anyhow::anyhow!("created object rotation is unavailable"))?;
    let position = state.position.map(|value| value as f32);
    let scale = state.scale.map(|value| value as f32);
    anyhow::ensure!(
        position.iter().chain(&scale).all(|value| value.is_finite()),
        "created object transform exceeds render precision"
    );
    let rotation_euler = if let [x, y, z] = state.rotation.as_slice() {
        [*x as f32, *y as f32, *z as f32]
    } else {
        let (x, y, z) = bevy::math::Quat::from_array(rotation).to_euler(bevy::math::EulerRot::XYZ);
        [x, y, z]
    };
    Ok(fe_runtime::messages::CommittedNodeMaterialization {
        node: fe_runtime::messages::NodeDto {
            node_id: state.node_id.clone(),
            petal_id: state.petal_id.clone(),
            name: state
                .display_name
                .clone()
                .ok_or_else(|| anyhow::anyhow!("created object name missing"))?,
            position,
            rotation,
            scale,
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
pub(crate) async fn create_object_granted(
    db: &Db,
    petal_id: &str,
    name: &str,
    asset_id: Option<&str>,
    position: [f32; 3],
    rotation: Vec<f64>,
    scale: Vec<f64>,
    properties: serde_json::Map<String, Value>,
    actor: &str,
    grant: &NodeAccessGrant,
) -> anyhow::Result<(String, Option<String>)> {
    let current = create_object_materialized_granted(
        db, petal_id, name, asset_id, position, rotation, scale, properties, actor, grant,
    )
    .await?;
    Ok((current.node.node_id, current.node.asset_path))
}

/// Create and return the complete predecoded current-state receipt only after commit.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_object_materialized_granted(
    db: &Db,
    petal_id: &str,
    name: &str,
    asset_id: Option<&str>,
    position: [f32; 3],
    rotation: Vec<f64>,
    scale: Vec<f64>,
    properties: serde_json::Map<String, Value>,
    actor: &str,
    grant: &NodeAccessGrant,
) -> anyhow::Result<fe_runtime::messages::CommittedNodeMaterialization> {
    let prepared = super::object_creation::prepare(
        db,
        super::object_creation::ObjectCreation {
            petal_id,
            name,
            asset: super::object_creation::SceneAsset::Existing(asset_id),
            position,
            rotation,
            scale,
            properties,
            actor,
            grant,
        },
    )
    .await?;
    super::object_creation::commit(db, prepared).await
}

/// Prepared ordinary facts and their authoritative resulting state.
pub(crate) struct PreparedChange {
    pub after: Value,
    pub version: i64,
    pub facts: Vec<(String, i64, Value)>,
}

/// Prepare the same baseline/reducer/version contract for one write or an atomic batch.
pub(crate) fn prepare_change(
    before: Value,
    previous: Option<&Value>,
    scope: &str,
    op: &str,
    data: Value,
    schema_versions: Value,
) -> anyhow::Result<PreparedChange> {
    let last_version = previous.map(row_version).transpose()?.unwrap_or(0);
    let continuous = previous.is_some_and(|row| {
        row["payload"]["journal_version"] == JOURNAL_VERSION
            && row["payload"]["scope"].as_str() == Some(scope)
            && row["payload"]["after_state"] == before
    });
    let baseline_version = last_version
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("node version exhausted"))?;
    let version = baseline_version
        .checked_add(i64::from(!continuous))
        .ok_or_else(|| anyhow::anyhow!("node version exhausted"))?;
    let mut after = before.clone();
    if op == "plugin_tombstone" {
        after["tombstone"] = data["marker"].clone();
    } else {
        reduce_change(&mut after, op, &data)?;
    }
    if op == "full_transform" {
        after["edit_seq"] = json!(
            before["edit_seq"]
                .as_i64()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("transform edit sequence exhausted"))?
        );
        after["elevation"] = data["position"][1].clone();
    }
    check_snapshot_size(&before)?;
    check_snapshot_size(&after)?;
    let mut facts = Vec::new();
    if !continuous {
        facts.push(("baseline".to_string(), baseline_version, json!({
            "journal_version": JOURNAL_VERSION, "scope": scope,
            "after_state": before, "state": before,
            "coverage_reason": if previous.is_none() { "first_covered_mutation" } else { "legacy_or_uncovered_interval" }
        })));
    }
    facts.push((
        op.to_string(),
        version,
        json!({"journal_version": JOURNAL_VERSION,
        "scope": scope, "data": data, "after_state": after, "schema_versions": schema_versions}),
    ));
    Ok(PreparedChange {
        after,
        version,
        facts,
    })
}

/// Commit the materialized change, intention, baseline and journal facts together.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_mutation(
    db: &Db,
    entry: OpLogEntry,
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    actor: &str,
    op: &str,
    data: Value,
    schema_versions: Value,
    grant: &NodeAccessGrant,
) -> anyhow::Result<i64> {
    let scope = authorized_node_scope(db, &entry.node_id.0, grant, actor).await?;
    commit_mutations_in_scope(
        db,
        entry,
        statements,
        bindings,
        actor,
        vec![(op.to_owned(), data, schema_versions)],
        scope,
    )
    .await
}

/// Commit journal changes to one node as ONE transaction and row version (QA-0129) for a
/// caller that already ran [`authorized_node_scope`] in this DB command: the lane is
/// serial, so the role cannot change between (QA-0161); the transaction fences the ancestry.
pub(crate) async fn commit_mutations_in_scope(
    db: &Db,
    entry: OpLogEntry,
    statements: Vec<String>,
    mut bindings: Vec<(String, Value)>,
    actor: &str,
    changes: Vec<(String, Value, Value)>,
    scope: String,
) -> anyhow::Result<i64> {
    anyhow::ensure!(!changes.is_empty(), "no journal change to commit");
    let node_id = entry.node_id.0.clone();
    let mut state = snapshot(db, &node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node not found"))?;
    let mut previous = latest(db, &node_id).await?;
    let mut facts = Vec::new();
    let mut version = 0;
    let atomic = changes.len() > 1;
    // Transform-only and tombstone writes skip the publishable check (scene_publication/AGENTS.md §row-bound).
    let publishable_before = changes
        .iter()
        .any(|(op, _, _)| !matches!(op.as_str(), "full_transform" | "plugin_tombstone"))
        .then(|| state.clone());
    for (op, data, schema_versions) in changes {
        let prepared = prepare_change(state, previous.as_ref(), &scope, &op, data, schema_versions)?;
        version = prepared.version;
        state = prepared.after;
        previous = prepared
            .facts
            .last()
            .map(|(_, version, payload)| json!({"row_version": version, "payload": payload}));
        facts.extend(prepared.facts);
    }
    // Every journaled writer: refuse a write whose row could never be published (a shrinking write passes).
    if let Some(before) = &publishable_before {
        super::scene_publication::check_publishable(db, &node_id, actor, Some(before), &state).await?;
    }
    if atomic {
        // Tag the facts as one atomic save so playback never shows an intermediate state (§atomic-replay).
        let save_id = ulid::Ulid::new().to_string();
        let last = facts.len() - 1;
        for (index, (op, _, payload)) in facts.iter_mut().enumerate() {
            if op != "baseline" {
                payload["atomic_save_id"] = json!(save_id);
            }
            if index == last {
                payload["atomic_save_request_id"] = json!(save_id);
                payload["editor_request_id"] = json!(save_id);
            }
        }
    }
    let actor = actor.to_string();
    let (mut scope_statements, scope_bindings) = scope_fence(&scope, Some(&node_id))?;
    commit_operation_atomic_with(db, entry, move |hlc| {
        scope_statements.extend(statements);
        let mut sql = scope_statements;
        bindings.extend(scope_bindings);
        for (index, (op, row_version, payload)) in facts.into_iter().enumerate() {
            let name = format!("history_fact_{index}");
            sql.push(format!("CREATE node_log CONTENT ${name}"));
            bindings.push((
                name,
                json!({"log_id": ulid::Ulid::new().to_string(),
                "node_id": node_id, "hlc_timestamp": hlc as i64, "op": op,
                "source_did": actor, "payload": payload, "row_version": row_version,
                "created_at": chrono::Utc::now().to_rfc3339()}),
            ));
        }
        (sql, bindings)
    })
    .await?;
    Ok(version)
}

fn reduce_change(state: &mut Value, op: &str, data: &Value) -> anyhow::Result<()> {
    match op {
        "node_renamed" => {
            let name = data["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty() && name.len() <= 1024)
                .ok_or_else(|| anyhow::anyhow!("invalid journal node name"))?;
            state["display_name"] = json!(name);
        }
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

/// Payload key a petal snapshot import stamps on every archive journal fact it restores (history, never proof).
pub(crate) const ARCHIVE_FACT_KEY: &str = "archive_import_id";

/// Require a completed local Save before publishing its resulting current state.
pub(crate) async fn require_committed_boundary(
    db: &Db,
    row: &Value,
    node_id: &str,
    actor: &str,
) -> anyhow::Result<()> {
    // An archive's own journal is never local publication proof (handlers/AGENTS.md §petal-snapshot-replication).
    anyhow::ensure!(
        row["payload"].get(ARCHIVE_FACT_KEY).is_none(),
        "an imported archive journal fact is never publication proof"
    );
    match atomic_save(db, row, node_id, &NodeAccessGrant::Local, actor).await? {
        AtomicSaveRead::Untagged => Ok(()),
        AtomicSaveRead::Saved(save) if row_version(row)? == save.committed_version => Ok(()),
        _ => anyhow::bail!("scene journal does not end at a completed Save"),
    }
}

/// Read a bounded immutable page or reconstruct one exact commit version.
pub(crate) async fn read_granted(
    db: &Db,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<Value> {
    if after_version < 0 || !(1..=200).contains(&limit) || at_version.is_some_and(|v| v < 1) {
        anyhow::bail!("invalid history bounds");
    }
    let latest = latest(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("history unavailable"))?;
    let current_scope = super::crud::resolve_node_scope_handler(db, node_id).await?;
    let admitted_scope = if let Some(scope) = current_scope {
        scope
    } else {
        let mut response = db.query("SELECT payload.scope AS scope, row_version FROM node_log WHERE node_id = $id AND payload.journal_version = 1 ORDER BY row_version DESC LIMIT 1")
            .bind(("id",node_id.to_string())).await?.check()?;
        let rows: Vec<Value> = response.take(0)?;
        rows.first()
            .and_then(|row| row["scope"].as_str())
            .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?
            .to_string()
    };
    authorize_scope(
        db,
        grant,
        local_did,
        &admitted_scope,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let current_version = row_version(&latest)?;
    let mut baseline_response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND op = 'baseline' AND payload.journal_version = 1 ORDER BY row_version ASC LIMIT 1")
        .bind(("id", node_id.to_string())).await?.check()?;
    let baselines: Vec<Value> = baseline_response.take(0)?;
    let baseline = match baselines.first() {
        Some(row) if fact_is_authorized(db, row, node_id, grant, local_did).await => Some(row),
        _ => None,
    };
    let coverage_start = baseline.map(row_version).transpose()?;
    let mut metadata = json!({"node_id":node_id, "journal_version": JOURNAL_VERSION,
        "current_version":current_version, "coverage_start_version":coverage_start,
        "coverage_started_at":baseline.and_then(|b|b.get("created_at")),
        "supported_operations":["baseline","property_set","property_deleted","full_transform","node_renamed"],
        "order":"per_object_commit_version", "event_time_ordering":false});
    let latest_authorized = fact_is_authorized(db, &latest, node_id, grant, local_did).await;
    let current_matches = snapshot(db, node_id)
        .await?
        .is_some_and(|state| latest_authorized && latest["payload"]["after_state"] == state);
    metadata["current_state_verified"] = json!(current_matches);
    if let Some(target) = at_version {
        return replay(db, node_id, target, metadata, grant, local_did).await;
    }
    // Byte-bounded page: up to `limit` entries and HISTORY_PAGE_BYTES, never empty (AGENTS.md §history-paging).
    let mut rows: Vec<Value> = Vec::new();
    let mut page_bytes = 0usize;
    let mut cursor = after_version;
    let more = loop {
        let want = (limit - rows.len()).min(HISTORY_FETCH_ROWS);
        let mut response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND row_version > $after ORDER BY row_version ASC LIMIT $limit")
            .bind(("id", node_id.to_string())).bind(("after", cursor)).bind(("limit", (want + 1) as i64)).await?.check()?;
        let batch: Vec<Value> = response.take(0)?;
        let read_more = batch.len() > want;
        let mut over_budget = false;
        for row in batch.into_iter().take(want) {
            let size = serde_json::to_vec(&row)?.len();
            if !rows.is_empty() && page_bytes + size > HISTORY_PAGE_BYTES {
                over_budget = true;
                break;
            }
            cursor = row_version(&row)?;
            page_bytes += size;
            rows.push(row);
        }
        if over_budget {
            break true;
        }
        if !read_more {
            break false;
        }
        if rows.len() == limit {
            break true;
        }
    };
    metadata["next_after_version"] = if more {
        rows.last()
            .map(row_version)
            .transpose()?
            .map_or(Value::Null, |v| json!(v))
    } else {
        Value::Null
    };
    let mut visible_rows = Vec::with_capacity(rows.len());
    let mut batches: BTreeMap<AtomicSaveTag, AtomicSaveRead> = BTreeMap::new();
    for mut row in rows {
        if fact_is_authorized(db, &row, node_id, grant, local_did).await {
            let tag = AtomicSaveTag::from_row(&row).ok().flatten();
            let boundary = match tag.as_ref().and_then(|tag| batches.get(tag)) {
                Some(boundary) => boundary.clone(),
                None => {
                    let boundary = atomic_save(db, &row, node_id, grant, local_did).await?;
                    if let Some(tag) = tag {
                        batches.insert(tag, boundary.clone());
                    }
                    boundary
                }
            };
            match boundary {
                AtomicSaveRead::Untagged => {}
                AtomicSaveRead::Saved(save) => {
                    row["replay_availability"] =
                        json!(if row_version(&row)? == save.committed_version {
                            "available"
                        } else {
                            "inside_atomic_batch"
                        });
                    row["atomic_save"] = serde_json::to_value(save)?;
                }
                AtomicSaveRead::Incomplete => {
                    row["replay_availability"] = json!("incomplete_atomic_batch")
                }
                AtomicSaveRead::UnavailableScope => {
                    visible_rows.push(unavailable_fact(node_id, row_version(&row)?));
                    continue;
                }
            }
            visible_rows.push(row);
        } else {
            visible_rows.push(unavailable_fact(node_id, row_version(&row)?));
        }
    }
    metadata["entries"] = json!(visible_rows);
    Ok(metadata)
}

async fn replay(
    db: &Db,
    node_id: &str,
    target: i64,
    mut metadata: Value,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<Value> {
    metadata["at_version"] = json!(target);
    let mut response = db.query("SELECT * OMIT id FROM node_log WHERE node_id = $id AND op = 'baseline' AND payload.journal_version = 1 AND row_version <= $target ORDER BY row_version DESC LIMIT 1")
        .bind(("id", node_id.to_string())).bind(("target", target)).await?.check()?;
    let baselines: Vec<Value> = response.take(0)?;
    let Some(baseline) = baselines.first() else {
        metadata["availability"] = json!("before_coverage");
        return Ok(metadata);
    };
    if !fact_is_authorized(db, baseline, node_id, grant, local_did).await {
        metadata["availability"] = json!("unavailable_scope");
        return Ok(metadata);
    }
    match atomic_save(db, baseline, node_id, grant, local_did).await? {
        AtomicSaveRead::Untagged => {}
        AtomicSaveRead::UnavailableScope => {
            metadata["availability"] = json!("unavailable_scope");
            return Ok(metadata);
        }
        AtomicSaveRead::Incomplete | AtomicSaveRead::Saved(_) => {
            metadata["availability"] = json!("incomplete_atomic_batch");
            return Ok(metadata);
        }
    }
    // Interval checks read light facts; the state comes from the target's stored after_state (AGENTS.md §history-replay).
    let start = row_version(baseline)?;
    let mut expected = start;
    let mut processed = 0;
    let mut batches: BTreeMap<AtomicSaveTag, AtomicSaveRead> = BTreeMap::new();
    while expected < target {
        let mut response = db.query(format!("{LIGHT_FACT_SELECT} WHERE node_id = $id AND row_version > $start AND row_version <= $target ORDER BY row_version ASC LIMIT 200 TIMEOUT 5s"))
            .bind(("id",node_id.to_string())).bind(("start",expected)).bind(("target",target)).await?.check()?;
        let rows: Vec<Value> = response.take::<Vec<Value>>(0)?.into_iter().map(light_fact).collect();
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            if !fact_is_authorized(db, row, node_id, grant, local_did).await {
                metadata["availability"] = json!("unavailable_scope");
                return Ok(metadata);
            }
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
            let tag = AtomicSaveTag::from_row(row).ok().flatten();
            let boundary = match tag.as_ref().and_then(|tag| batches.get(tag)) {
                Some(boundary) => boundary.clone(),
                None => {
                    let boundary = atomic_save(db, row, node_id, grant, local_did).await?;
                    if let Some(tag) = tag {
                        batches.insert(tag, boundary.clone());
                    }
                    boundary
                }
            };
            match boundary {
                AtomicSaveRead::Untagged => {}
                AtomicSaveRead::Saved(save) => {
                    if target < save.committed_version {
                        metadata["availability"] = json!("inside_atomic_batch");
                        metadata["atomic_save"] = serde_json::to_value(save)?;
                        return Ok(metadata);
                    }
                    if target == save.committed_version {
                        metadata["atomic_save"] = serde_json::to_value(save)?;
                    }
                }
                AtomicSaveRead::Incomplete => {
                    metadata["availability"] = json!("incomplete_atomic_batch");
                    return Ok(metadata);
                }
                AtomicSaveRead::UnavailableScope => {
                    metadata["availability"] = json!("unavailable_scope");
                    return Ok(metadata);
                }
            }
            if !row["op"].as_str().is_some_and(|op| REPLAYABLE_OPERATIONS.contains(&op)) {
                metadata["availability"] = json!("unsupported_operation");
                return Ok(metadata);
            }
        }
    }
    if expected != target {
        metadata["availability"] = json!("version_unavailable");
        return Ok(metadata);
    }
    // Every fact stores the full state after it: the baseline's own state, or the target's after_state.
    let state = if target == start {
        baseline["payload"]["state"].clone()
    } else {
        let mut response = db.query("SELECT VALUE payload.after_state FROM node_log WHERE node_id = $id AND row_version = $target LIMIT 1")
            .bind(("id", node_id.to_string())).bind(("target", target)).await?.check()?;
        let states: Vec<Value> = response.take(0)?;
        match states.into_iter().next() {
            Some(state) if state.is_object() => state,
            _ => {
                metadata["availability"] = json!("incomplete_interval");
                return Ok(metadata);
            }
        }
    };
    metadata["availability"] = json!("available");
    metadata["state"] = state;
    metadata["baseline_version"] = json!(start);
    Ok(metadata)
}

/// Internal local create compatibility seam.
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
    create_object_granted(
        db,
        petal_id,
        name,
        asset_id,
        position,
        rotation,
        scale,
        properties,
        actor,
        &NodeAccessGrant::Local,
    )
    .await
}

/// Internal local read compatibility seam.
#[cfg(test)]
pub(crate) async fn read(
    db: &Db,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
    local_did: &str,
) -> anyhow::Result<Value> {
    read_granted(
        db,
        node_id,
        after_version,
        limit,
        at_version,
        &NodeAccessGrant::Local,
        local_did,
    )
    .await
}
