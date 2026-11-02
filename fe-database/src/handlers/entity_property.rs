use tracing::instrument;

use fe_query::{Filter, QueryBuilder};

use crate::handlers::preconditions::require_node_exists;
use crate::query_helpers::exec_query;
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
    super::node_history::authorized_node_scope(db, node_id, grant).await?;

    let schema_versions = super::field_def::validate_property(db, node_id, key, value).await?;
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
        vec!["UPDATE node SET properties[$key] = $val WHERE node_id = $node_id".into()],
        vec![
            ("node_id".into(), serde_json::json!(node_id)),
            ("key".into(), serde_json::json!(key)),
            ("val".into(), value.clone()),
        ],
        actor,
        "property_set",
        data,
        schema_versions,
        grant,
    )
    .await?;
    Ok(())
}

/// Get all custom properties of a node.
///
/// Returns the `properties` JSON object, or `{}` if the node has no properties.
#[instrument(skip(db))]
pub(crate) async fn get_entity_properties_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<serde_json::Value> {
    let q = QueryBuilder::new()
        .select(&["properties"])
        .from("node")
        .filter(Filter::eq("node_id", node_id))
        .limit(1)
        .build();
    let mut res = exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("GetNodeProperties DB query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("GetNodeProperties take failed: {e}"))?;

    if rows.is_empty() {
        tracing::warn!(node_id, "GetNodeProperties matched no node — id mismatch?");
    }
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
    super::node_history::authorized_node_scope(db, node_id, grant).await?;

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

/// Local compatibility entry point for node-view writes.
pub(crate) async fn set_entity_property_handler(
    db: &Db,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    set_entity_property_as(db, node_id, key, value, "local").await
}

/// Local compatibility entry point for node-view removals.
pub(crate) async fn delete_entity_property_handler(
    db: &Db,
    node_id: &str,
    key: &str,
) -> anyhow::Result<()> {
    delete_entity_property_as(db, node_id, key, "local").await
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
