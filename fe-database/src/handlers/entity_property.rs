use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::handlers::preconditions::require_node_exists;
use crate::op_log::commit_operation;
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
    require_node_exists(db, node_id, "SetNodeProperty").await?;

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
    commit_operation(db, entry, |_| async {
        let q = UpdateBuilder::update("node")
            .set_expr("properties[$key]", "$val")
            .where_clause(Filter::eq("node_id", node_id))
            .build();
        let mut query = db.query(&q.sql);
        for (name, val) in &q.params {
            query = query.bind((name.clone(), val.clone()));
        }
        let mut res = query
            .bind(("key", key.to_string()))
            .bind(("val", value.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("SetNodeProperty DB query failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("SetNodeProperty statement failed: {e}"))?;
        let updated: Vec<serde_json::Value> = res
            .take(0)
            .map_err(|e| anyhow::anyhow!("SetNodeProperty take failed: {e}"))?;
        if updated.is_empty() {
            anyhow::bail!("SetNodeProperty matched no node with node_id = {node_id}");
        }
        tracing::info!(
            node_id,
            key,
            rows = updated.len(),
            "SetNodeProperty persisted"
        );
        Ok(())
    })
    .await
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
pub(crate) async fn delete_entity_property_handler(
    db: &Db,
    node_id: &str,
    key: &str,
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "DeleteNodeProperty").await?;

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
    commit_operation(db, entry, |_| async {
        let q = UpdateBuilder::update("node")
            .set_expr("properties", "object::remove(properties ?? {}, $key)")
            .where_clause(Filter::eq("node_id", node_id))
            .build();
        let mut query = db.query(&q.sql);
        for (name, val) in &q.params {
            query = query.bind((name.clone(), val.clone()));
        }
        let mut res = query
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DeleteNodeProperty DB query failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("DeleteNodeProperty statement failed: {e}"))?;
        let updated: Vec<serde_json::Value> = res
            .take(0)
            .map_err(|e| anyhow::anyhow!("DeleteNodeProperty take failed: {e}"))?;
        if updated.is_empty() {
            anyhow::bail!("DeleteNodeProperty matched no node with node_id = {node_id}");
        }
        Ok(())
    })
    .await
}
