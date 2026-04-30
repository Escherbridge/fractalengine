use crate::repo::Db;

/// Persist a transform change for a node (position/rotation/scale).
pub(crate) async fn update_node_transform_handler(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<()> {
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
    let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
    db.query(
        "UPDATE node SET \
            position = <geometry<point>> [$x, $z], \
            elevation = $y, \
            rotation = $rot, \
            scale = $sc \
         WHERE node_id = $node_id",
    )
    .bind(("node_id", node_id.to_string()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("rot", rot))
    .bind(("sc", sc))
    .await
    .map_err(|e| anyhow::anyhow!("UpdateNodeTransform DB query failed: {e}"))?;
    Ok(())
}

/// Persist a portal URL change for a node's associated model.
pub(crate) async fn update_node_url_handler(
    db: &Db,
    node_id: &str,
    url: Option<String>,
) -> anyhow::Result<()> {
    db.query(
        "UPDATE model SET external_url = $url \
         WHERE asset_id = (SELECT VALUE asset_id FROM node WHERE node_id = $node_id LIMIT 1)[0]",
    )
    .bind(("node_id", node_id.to_string()))
    .bind(("url", url))
    .await
    .map_err(|e| anyhow::anyhow!("UpdateNodeUrl DB query failed: {e}"))?;
    Ok(())
}
