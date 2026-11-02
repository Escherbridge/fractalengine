use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::handlers::preconditions::require_node_exists;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Persist a transform change for a node (position/rotation/scale).
#[instrument(skip(db))]
pub(crate) async fn update_node_transform_as(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    actor: &str,
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "UpdateNodeTransform").await?;
    if position
        .iter()
        .chain(&rotation)
        .chain(&scale)
        .any(|v| !v.is_finite())
    {
        anyhow::bail!("transform components must be finite");
    }
    reject_stamp_translate(db, node_id, position).await?;

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
    let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
    let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
    super::node_history::commit_mutation(db, entry,
        vec!["UPDATE node SET position = <geometry<point>> [$x, $z], elevation = $y, rotation = $rot, scale = $sc, edit_seq = edit_seq + 1 WHERE node_id = $node_id".into()],
        vec![("x".into(), serde_json::json!(position[0] as f64)), ("z".into(), serde_json::json!(position[2] as f64)), ("y".into(), serde_json::json!(position[1] as f64)),
            ("rot".into(), serde_json::json!(rot)), ("sc".into(), serde_json::json!(sc)), ("node_id".into(), serde_json::json!(node_id))],
        actor, "full_transform", serde_json::json!({"position":position,"rotation":rotation,"scale":scale}), serde_json::json!({})).await?;
    Ok(())
}

/// Max per-axis drift treated as "position unchanged" for the stamp lock.
const STAMP_POSITION_EPSILON: f64 = 1e-4;

/// D-A5 position lock: a `node_kind = 'stamp'` row is path-derived, so a
/// request that moves it fails BEFORE the op-log commit (no row, no op).
/// Rotation/scale-only updates pass. See src/AGENTS.md §lifecycle.
async fn reject_stamp_translate(db: &Db, node_id: &str, position: [f32; 3]) -> anyhow::Result<()> {
    let mut res = db
        .query(
            "SELECT properties.node_kind AS node_kind, position, elevation FROM node \
             WHERE node_id = $nid LIMIT 1",
        )
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("UpdateNodeTransform stamp-lock read failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("UpdateNodeTransform stamp-lock take failed: {e}"))?;
    let Some(row) = rows.first() else {
        return Ok(());
    };
    if row["node_kind"].as_str() != Some("stamp") {
        return Ok(());
    }
    let coords = &row["position"]["coordinates"];
    let stored = [
        coords[0].as_f64().unwrap_or(0.0),
        row["elevation"].as_f64().unwrap_or(0.0),
        coords[1].as_f64().unwrap_or(0.0),
    ];
    let moved = stored
        .iter()
        .zip(position)
        .any(|(stored, requested)| (f64::from(requested) - stored).abs() > STAMP_POSITION_EPSILON);
    if moved {
        anyhow::bail!(
            "position-locked: stamp {node_id} is path-derived (D-A5); only rotation/scale may change"
        );
    }
    Ok(())
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
