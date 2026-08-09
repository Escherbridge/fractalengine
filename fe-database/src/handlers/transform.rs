use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::handlers::preconditions::require_node_exists;
use crate::op_log::commit_operation;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Persist a transform change for a node (position/rotation/scale).
#[instrument(skip(db))]
pub(crate) async fn update_node_transform_handler(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "UpdateNodeTransform").await?;

    let payload = serde_json::json!({
        "node_id": node_id,
        "position": position,
        "rotation": rotation,
        "scale": scale,
    });

    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::TransformUpdate,
        payload,
        sig: "00".repeat(64),
    };
    commit_operation(db, entry, |_| async {
        // Geometry casts are required for SCHEMAFULL node writes; see AGENTS.md §geometry-inserts.
        let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
        let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
        let mut result = db
            .query(
                "UPDATE node SET
                position = <geometry<point>> [$x, $z],
                elevation = $y,
                rotation = $rot,
                scale = $sc,
                edit_seq = edit_seq + 1
            WHERE node_id = $node_id",
            )
            .bind(("x", position[0] as f64))
            .bind(("z", position[2] as f64))
            .bind(("y", position[1] as f64))
            .bind(("rot", rot))
            .bind(("sc", sc))
            .bind(("node_id", node_id.to_string()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("UpdateNodeTransform DB query failed: {e}"))?;
        let updated: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| anyhow::anyhow!("UpdateNodeTransform result read failed: {e}"))?;
        if updated.is_empty() {
            anyhow::bail!("UpdateNodeTransform matched no node with node_id = {node_id}");
        }
        Ok(())
    })
    .await
}

/// Persist a portal URL change for a node's associated model.
#[instrument(skip(db))]
pub(crate) async fn update_node_url_handler(
    db: &Db,
    node_id: &str,
    url: Option<String>,
) -> anyhow::Result<()> {
    // Step 1: look up the node's asset_id
    let q = QueryBuilder::new()
        .select(&["asset_id"])
        .from("node")
        .filter(Filter::eq("node_id", node_id))
        .limit(1)
        .build();
    let mut res = exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("UpdateNodeUrl: asset lookup failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    let asset_id = rows
        .first()
        .and_then(|r| r["asset_id"].as_str())
        .ok_or_else(|| anyhow::anyhow!("UpdateNodeUrl: node {node_id} has no asset_id"))?
        .to_string();

    // Step 2: update the model's external_url
    let uq = UpdateBuilder::update("model")
        .set(
            "external_url",
            url.map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        )
        .where_clause(Filter::eq("asset_id", asset_id.as_str()))
        .build();
    exec_query(db, &uq)
        .await
        .map_err(|e| anyhow::anyhow!("UpdateNodeUrl DB query failed: {e}"))?;
    Ok(())
}
