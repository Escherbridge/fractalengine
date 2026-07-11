use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::op_log::write_op_log;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Persist a transform change for a node (position/rotation/scale).
///
/// Before updating, queries the current transform to build an op_log entry
/// with both old and new values. This enables:
/// - **Undo**: reverse-apply the old values
/// - **P2P merge**: compare HLC timestamps to resolve conflicts
#[instrument(skip(db))]
pub(crate) async fn update_node_transform_handler(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<()> {
    // Step 1: Query current transform for op_log (old values)
    let old_transform = query_current_transform(db, node_id).await;

    // Step 2: Write op_log entry BEFORE the mutation
    let payload = serde_json::json!({
        "node_id": node_id,
        "old_position": old_transform.as_ref().map(|(p, _, _)| p).copied().unwrap_or([0.0; 3]),
        "old_rotation": old_transform.as_ref().map(|(_, r, _)| r).copied().unwrap_or([0.0; 3]),
        "old_scale": old_transform.as_ref().map(|(_, _, s)| s).copied().unwrap_or([1.0, 1.0, 1.0]),
        "new_position": position,
        "new_rotation": rotation,
        "new_scale": scale,
    });

    let entry = OpLogEntry {
        lamport_clock: 0, // populated by write_op_log via HLC
        hlc_timestamp: String::new(), // populated by write_op_log
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::TransformUpdate,
        payload,
        sig: "00".repeat(64),
    };
    if let Err(e) = write_op_log(db, entry).await {
        tracing::warn!("Failed to write transform op_log for {node_id}: {e}");
        // Don't fail the transform — op_log is best-effort for now
    }

    // Step 3: perform the UPDATE — geometry needs the explicit cast; see AGENTS.md §geometry-inserts.
    let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
    let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
    db.query(
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
    Ok(())
}

/// Query the current transform of a node (position, rotation, scale).
/// Returns None if the node doesn't exist. Used for op_log old-value capture.
async fn query_current_transform(
    db: &Db,
    node_id: &str,
) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    let q = QueryBuilder::new()
        .select(&["position", "elevation", "rotation", "scale"])
        .from("node")
        .filter(Filter::eq("node_id", node_id))
        .limit(1)
        .build();
    let mut res = exec_query(db, &q).await.ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let row = rows.first()?;

    let coords = &row["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0) as f32;
    let z = coords[1].as_f64().unwrap_or(0.0) as f32;
    let y = row["elevation"].as_f64().unwrap_or(0.0) as f32;

    let rotation_raw = &row["rotation"];
    let rotation = if rotation_raw.is_array() {
        let arr = rotation_raw.as_array().unwrap();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        ]
    } else {
        [0.0, 0.0, 0.0]
    };

    let scale_raw = &row["scale"];
    let scale = if scale_raw.is_array() {
        let arr = scale_raw.as_array().unwrap();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
        ]
    } else {
        [1.0, 1.0, 1.0]
    };

    Some(([x, y, z], rotation, scale))
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
    let mut res = exec_query(db, &q).await
        .map_err(|e| anyhow::anyhow!("UpdateNodeUrl: asset lookup failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    let asset_id = rows
        .first()
        .and_then(|r| r["asset_id"].as_str())
        .ok_or_else(|| anyhow::anyhow!("UpdateNodeUrl: node {node_id} has no asset_id"))?
        .to_string();

    // Step 2: update the model's external_url
    let uq = UpdateBuilder::update("model")
        .set("external_url", url.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null))
        .where_clause(Filter::eq("asset_id", asset_id.as_str()))
        .build();
    exec_query(db, &uq).await
        .map_err(|e| anyhow::anyhow!("UpdateNodeUrl DB query failed: {e}"))?;
    Ok(())
}
