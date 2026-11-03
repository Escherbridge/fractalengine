use tracing::instrument;

use crate::handlers::preconditions::require_node_exists;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Set (or update) a custom property on a node.
///
/// Merges the given key/value into the node's `properties` JSON object.
/// If the node has no properties yet, initialises the object.
#[instrument(skip(db))]
pub(crate) async fn set_entity_property_granted(
    db: &Db,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
    actor: &str,
    grant: &fe_runtime::node_history::NodeAccessGrant,
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "SetNodeProperty").await?;
    super::node_history::authorized_node_scope(db, node_id, grant, actor).await?;

    let schema_versions = super::field_def::validate_property(db, node_id, key, value).await?;
    let (mut statements, mut bindings) = if key == super::node_view::VIEW_GROUPS_KEY {
        super::node_view::membership_fence(db, node_id, value).await?
    } else {
        (Vec::new(), Vec::new())
    };
    statements.push("UPDATE node SET properties[$key] = $val WHERE node_id = $node_id".into());
    bindings.extend([
        ("node_id".into(), serde_json::json!(node_id)),
        ("key".into(), serde_json::json!(key)),
        ("val".into(), value.clone()),
    ]);
    let data = serde_json::json!({"key":key,"value":value});
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::PropertySet,
        payload: serde_json::json!({"node_id":node_id,"key":key,"value":value}),
        sig: "00".repeat(64),
    };
    super::node_history::commit_mutation(
        db,
        entry,
        statements,
        bindings,
        actor,
        "property_set",
        data,
        schema_versions,
        grant,
    )
    .await?;
    Ok(())
}

/// Read one live node's custom properties, preserving an unset bag as `{}`.
#[instrument(skip(db))]
pub(crate) async fn get_entity_properties_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut res = db
        .query("SELECT properties FROM node WHERE node_id=$node AND tombstone=NONE LIMIT 2")
        .bind(("node", node_id.to_owned()))
        .await
        .map_err(|e| anyhow::anyhow!("GetNodeProperties DB query failed: {e}"))?
        .check()?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("GetNodeProperties take failed: {e}"))?;

    anyhow::ensure!(
        rows.len() == 1,
        "GetNodeProperties: live node unavailable or ambiguous"
    );
    let properties = rows
        .first()
        .and_then(|r| r.get("properties"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // If properties is null (not set yet), return empty object
    if properties.is_null() {
        return Ok(serde_json::json!({}));
    }

    Ok(properties)
}

/// Delete a custom property from a node.
///
/// Removes the specified key from the node's `properties` JSON object.
#[instrument(skip(db))]
pub(crate) async fn delete_entity_property_granted(
    db: &Db,
    node_id: &str,
    key: &str,
    actor: &str,
    grant: &fe_runtime::node_history::NodeAccessGrant,
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "DeleteNodeProperty").await?;
    super::node_history::authorized_node_scope(db, node_id, grant, actor).await?;

    let data = serde_json::json!({"key":key});
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::PropertyDeleted,
        payload: serde_json::json!({"node_id":node_id,"key":key}),
        sig: "00".repeat(64),
    };
    super::node_history::commit_mutation(db, entry,
        vec!["UPDATE node SET properties = object::remove(properties ?? {}, $key) WHERE node_id = $node_id".into()],
        vec![("node_id".into(), serde_json::json!(node_id)), ("key".into(), serde_json::json!(key))],
        actor, "property_deleted", data, serde_json::json!({}), grant).await?;
    Ok(())
}

/// Internal local property write compatibility entry.
pub(crate) async fn set_entity_property_as(
    db: &Db,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
    actor: &str,
) -> anyhow::Result<()> {
    set_entity_property_granted(
        db,
        node_id,
        key,
        value,
        actor,
        &fe_runtime::node_history::NodeAccessGrant::Local,
    )
    .await
}

/// Internal local property removal compatibility entry.
pub(crate) async fn delete_entity_property_as(
    db: &Db,
    node_id: &str,
    key: &str,
    actor: &str,
) -> anyhow::Result<()> {
    delete_entity_property_granted(
        db,
        node_id,
        key,
        actor,
        &fe_runtime::node_history::NodeAccessGrant::Local,
    )
    .await
}
