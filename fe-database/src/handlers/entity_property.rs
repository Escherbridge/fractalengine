use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::op_log::write_op_log;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Set (or update) a custom property on a node.
///
/// Merges the given key/value into the node's `properties` JSON object.
/// If the node has no properties yet, initialises the object.
#[instrument(skip(db))]
pub(crate) async fn set_entity_property_handler(
    db: &Db,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    // SurrealDB 3.x idiomatic dynamic-key setter: `properties[$key] = $val`
    // The builder's set() uses parameterized field names, but here the field
    // itself contains a dynamic key. Use set_expr for the dynamic path and
    // manually bind key/val via raw query (builder lacks dynamic-key support).
    // TODO(fe-query): migrate fully when dynamic-key set support is added.
    let q = UpdateBuilder::update("node")
        .set_expr("properties[$key]", "$val")
        .where_clause(Filter::eq("node_id", node_id))
        .build();
    // We need extra binds for $key and $val which are in the set_expr
    let mut query = db.query(&q.sql);
    for (name, val) in &q.params {
        query = query.bind((name.clone(), val.clone()));
    }
    query = query.bind(("key", key.to_string()));
    query = query.bind(("val", value.clone()));
    query
        .await
        .map_err(|e| anyhow::anyhow!("SetNodeProperty DB query failed: {e}"))?;

    // Write op_log entry
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::PropertySet,
        payload: serde_json::json!({
            "node_id": node_id,
            "key": key,
            "value": value,
        }),
        sig: "00".repeat(64),
    };
    if let Err(e) = write_op_log(db, entry).await {
        tracing::warn!("Failed to write property set op_log for {node_id}: {e}");
    }

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
pub(crate) async fn delete_entity_property_handler(
    db: &Db,
    node_id: &str,
    key: &str,
) -> anyhow::Result<()> {
    // Use object::remove to delete the key from properties — raw expr needed
    let q = UpdateBuilder::update("node")
        .set_expr("properties", "object::remove(properties ?? {}, $key)")
        .where_clause(Filter::eq("node_id", node_id))
        .build();
    // Bind the extra $key parameter used inside the set_expr
    let mut query = db.query(&q.sql);
    for (name, val) in &q.params {
        query = query.bind((name.clone(), val.clone()));
    }
    query = query.bind(("key", key.to_string()));
    query
        .await
        .map_err(|e| anyhow::anyhow!("DeleteNodeProperty DB query failed: {e}"))?;

    // Write op_log entry
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::PropertyDeleted,
        payload: serde_json::json!({
            "node_id": node_id,
            "key": key,
        }),
        sig: "00".repeat(64),
    };
    if let Err(e) = write_op_log(db, entry).await {
        tracing::warn!("Failed to write property delete op_log for {node_id}: {e}");
    }

    Ok(())
}
