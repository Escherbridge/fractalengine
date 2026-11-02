use tracing::instrument;

use crate::handlers::preconditions::require_node_exists;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// Persist a transform change for a node (position/rotation/scale).
#[instrument(skip(db))]
pub(crate) async fn update_node_transform_granted(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    actor: &str,
    grant: &fe_runtime::node_history::NodeAccessGrant,
) -> anyhow::Result<()> {
    require_node_exists(db, node_id, "UpdateNodeTransform").await?;
    super::node_history::authorized_node_scope(db, node_id, grant, actor).await?;
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
        actor, "full_transform", serde_json::json!({"position":position,"rotation":rotation,"scale":scale}), serde_json::json!({}), grant).await?;
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

/// Commit an authorized local model URL write and return its stored row.
#[instrument(skip(db, url, local_did))]
pub(crate) async fn update_node_url_handler(
    db: &Db,
    node_id: &str,
    url: Option<String>,
    expected_scope: &str,
    local_did: &str,
) -> anyhow::Result<serde_json::Value> {
    use serde_json::json;
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Node is missing or unavailable"))?;
    anyhow::ensure!(
        scope == expected_scope,
        "Node moved to another Petal; reload before saving"
    );
    anyhow::ensure!(
        url.as_ref()
            .is_none_or(|url| url.len() <= 8192 && !url.chars().any(char::is_control)),
        "Invalid URL length or control characters"
    );
    let access = super::entity::prepare_scope_permission(
        db,
        &scope,
        local_did,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let parts = crate::parse_scope(&scope)?;
    let petal = parts
        .petal_id
        .ok_or_else(|| anyhow::anyhow!("Missing Petal scope"))?;
    let mut snapshot = db.query("SELECT node_id, petal_id, asset_id, tombstone FROM node WHERE node_id = $url_node; SELECT asset_id, petal_id, external_url FROM model WHERE asset_id IN (SELECT VALUE asset_id FROM node WHERE node_id = $url_node)")
        .bind(("url_node", node_id.to_owned())).await?.check()?;
    let nodes: Vec<serde_json::Value> = snapshot.take(0)?;
    let models: Vec<serde_json::Value> = snapshot.take(1)?;
    anyhow::ensure!(
        nodes.len() == 1
            && nodes[0]["petal_id"].as_str() == Some(petal.as_str())
            && nodes[0]
                .get("tombstone")
                .is_none_or(serde_json::Value::is_null),
        "Node is missing, removed, or moved"
    );
    let asset = nodes[0]["asset_id"]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!("This node has no placed model; a Portal URL cannot be stored on it")
        })?
        .to_owned();
    anyhow::ensure!(
        models.len() == 1 && models[0]["petal_id"].as_str() == Some(petal.as_str()),
        "Placed model is missing, shared, or in another Petal"
    );
    let (mut statements, mut bindings) = access.into_parts();
    statements.extend([
        "IF (SELECT node_id, petal_id, asset_id, tombstone FROM node WHERE node_id = $url_node) != $url_nodes { THROW 'URL node changed'; }".into(),
        "IF (SELECT asset_id, petal_id, external_url FROM model WHERE asset_id = $url_asset) != $url_models { THROW 'URL model changed'; }".into(),
        "IF (SELECT VALUE node_id FROM node WHERE asset_id = $url_asset AND tombstone = NONE) != [$url_node] { THROW 'URL model is shared; select an independent model'; }".into(),
    ]);
    bindings.extend([
        ("url_node".into(), json!(node_id)),
        ("url_nodes".into(), json!(nodes)),
        ("url_asset".into(), json!(asset)),
        ("url_models".into(), json!(models)),
    ]);
    statements.push(
        if url.is_some() {
            "UPDATE model SET external_url = $url_value WHERE asset_id = $url_asset RETURN AFTER"
        } else {
            "UPDATE model SET external_url = NONE WHERE asset_id = $url_asset RETURN AFTER"
        }
        .into(),
    );
    if let Some(url) = &url {
        bindings.push(("url_value".into(), json!(url)));
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_owned()),
        op_type: OpType::UpdateModelMeta,
        payload: json!({"asset_id":asset,"external_url":url}),
        sig: "00".repeat(64),
    };
    let (_, mut rows) =
        crate::op_log::commit_operation_atomic(db, entry, &statements, bindings).await?;
    let rows = rows.pop().unwrap_or_default();
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Committed URL has no readback; reload the object"))?;
    anyhow::ensure!(
        row.get("external_url").and_then(serde_json::Value::as_str) == url.as_deref(),
        "Committed URL readback differs; reload the object"
    );
    Ok(row)
}

/// Internal local transform write compatibility entry.
pub(crate) async fn update_node_transform_as(
    db: &Db,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    actor: &str,
) -> anyhow::Result<()> {
    update_node_transform_granted(
        db,
        node_id,
        position,
        rotation,
        scale,
        actor,
        &fe_runtime::node_history::NodeAccessGrant::Local,
    )
    .await
}
