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
    let scope = super::node_history::authorized_node_scope(db, node_id, grant, actor).await?;
    if position
        .iter()
        .chain(&rotation)
        .chain(&scale)
        .any(|v| !v.is_finite())
    {
        anyhow::bail!("transform components must be finite");
    }
    let is_stamp = reject_stamp_translate(db, node_id, position).await?;
    let stamp_mirror = if is_stamp {
        Some(super::stamp_transform::mirror_overrides(db, &scope, rotation, scale).await?)
    } else {
        None
    };

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
    let mut statements = vec!["UPDATE node SET position = <geometry<point>> [$x, $z], elevation = $y, rotation = $rot, scale = $sc, edit_seq = edit_seq + 1 WHERE node_id = $node_id".to_string()];
    let mut bindings: Vec<(String, serde_json::Value)> = vec![("x".into(), serde_json::json!(position[0] as f64)), ("z".into(), serde_json::json!(position[2] as f64)), ("y".into(), serde_json::json!(position[1] as f64)),
        ("rot".into(), serde_json::json!(rot)), ("sc".into(), serde_json::json!(sc)), ("node_id".into(), serde_json::json!(node_id))];
    let mut changes = vec![("full_transform".to_string(), serde_json::json!({"position":position,"rotation":rotation,"scale":scale}), serde_json::json!({}))];
    // A stamp renders from its overrides, so they commit in the same transaction as the row.
    if let Some(mirror) = stamp_mirror {
        statements.extend(mirror.statements);
        bindings.extend(mirror.bindings);
        changes.extend(mirror.changes);
    }
    super::node_history::commit_mutations_in_scope(db, entry, statements, bindings, actor, changes, scope).await?;
    Ok(())
}

/// Max per-axis drift treated as "position unchanged" for the stamp lock.
const STAMP_POSITION_EPSILON: f64 = 1e-4;

/// D-A5 position lock: a `node_kind = 'stamp'` row is path-derived, so a
/// request that moves it fails BEFORE the op-log commit (no row, no op).
/// Rotation/scale-only updates pass and return `true` for a stamp row. See src/AGENTS.md §lifecycle.
async fn reject_stamp_translate(db: &Db, node_id: &str, position: [f32; 3]) -> anyhow::Result<bool> {
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
        return Ok(false);
    };
    if row["node_kind"].as_str() != Some("stamp") {
        return Ok(false);
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
    Ok(true)
}

// Canonical presence/value snapshots preserve NONE across JSON bindings.
const URL_NODE_COLUMNS: &str = "node_id, petal_id, (asset_id = NONE OR asset_id = NULL) AS asset_absent, IF asset_id = NONE OR asset_id = NULL { '' } ELSE { asset_id } AS asset_value, (tombstone = NONE OR tombstone = NULL) AS live";
const URL_MODEL_COLUMNS: &str = "asset_id, petal_id, (external_url = NONE OR external_url = NULL) AS url_absent, IF external_url = NONE OR external_url = NULL { 0 } ELSE { string::len(external_url) } AS url_length, IF external_url = NONE OR external_url = NULL { '' } ELSE { string::slice(external_url, 0, 8193) } AS url_value";

fn captured_url_asset(nodes: &[serde_json::Value], petal: &str) -> anyhow::Result<Option<String>> {
    anyhow::ensure!(
        nodes.len() == 1
            && nodes[0]["petal_id"].as_str() == Some(petal)
            && nodes[0]["live"].as_bool() == Some(true),
        "URL node is missing, removed or moved"
    );
    match (
        nodes[0]["asset_absent"].as_bool(),
        nodes[0]["asset_value"].as_str(),
    ) {
        (Some(true), Some("")) => Ok(None),
        (Some(false), Some(asset)) => Ok(Some(asset.to_owned())),
        _ => anyhow::bail!("URL node asset metadata is unsupported"),
    }
}

fn captured_model_url(row: &serde_json::Value) -> anyhow::Result<Option<String>> {
    anyhow::ensure!(
        row["url_length"]
            .as_u64()
            .is_some_and(|length| length <= 8192),
        "Stored URL exceeds the read limit"
    );
    match (row["url_absent"].as_bool(), row["url_value"].as_str()) {
        (Some(true), Some("")) => Ok(None),
        (Some(false), Some(value)) if value.len() <= 8192 => Ok(Some(value.to_owned())),
        _ => anyhow::bail!("Stored URL has an unsupported value or exceeds the read limit"),
    }
}

/// Select a useful transaction error without exposing stored URL data.
pub(crate) fn url_read_transaction_error(
    errors: std::collections::HashMap<usize, surrealdb::Error>,
    model_index: usize,
) -> anyhow::Error {
    let mut errors: Vec<_> = errors.into_iter().collect();
    errors.sort_by_key(|(index, _)| *index);
    let selected = errors
        .iter()
        .find(|(_, error)| !error.to_string().contains("due to a failed transaction"))
        .or_else(|| errors.first());
    let Some((index, error)) = selected else {
        return anyhow::anyhow!("URL read transaction failed");
    };
    let message = error.to_string();
    let reason = [
        "URL node identity changed",
        "hierarchy authority changed",
        "ambiguous hierarchy identity",
        "hierarchy ancestry changed",
    ]
    .into_iter()
    .find(|reason| message.contains(reason))
    .unwrap_or(if *index == model_index {
        "model projection failed"
    } else {
        "authority or identity fence failed"
    });
    anyhow::anyhow!("URL read transaction statement {index}: {reason}")
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
    prepare_node_url_write(db, node_id, url, expected_scope, local_did)
        .await?
        .commit(db)
        .await
}

/// A sealed local URL mutation; see handlers/AGENTS.md section Portal URL transaction.
pub(crate) struct PreparedUrlWrite {
    statements: Vec<String>,
    bindings: Vec<(String, serde_json::Value)>,
    entry: OpLogEntry,
    url: Option<String>,
}

impl PreparedUrlWrite {
    pub(crate) async fn commit(self, db: &Db) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            self.url
                .as_deref()
                .is_none_or(fe_runtime::portal_url::embedded_allowed),
            "Portal fixture capability expired before commit"
        );
        let (_, mut rows) =
            crate::op_log::commit_operation_atomic(db, self.entry, &self.statements, self.bindings)
                .await?;
        let rows = rows.pop().unwrap_or_default();
        anyhow::ensure!(
            rows.len() == 1,
            "Committed URL has no unique readback; reload the object"
        );
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Committed URL readback is unavailable"))?;
        anyhow::ensure!(
            row.get("external_url").and_then(serde_json::Value::as_str) == self.url.as_deref(),
            "Committed URL readback differs; reload the object"
        );
        Ok(row)
    }
}

pub(crate) async fn prepare_node_url_write(
    db: &Db,
    node_id: &str,
    url: Option<String>,
    expected_scope: &str,
    local_did: &str,
) -> anyhow::Result<PreparedUrlWrite> {
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
    anyhow::ensure!(
        url.as_deref()
            .is_none_or(fe_runtime::portal_url::embedded_allowed),
        "Portal URL blocked by security policy or unavailable fixture helper"
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
    let mut snapshot = db
        .query(format!(
            "SELECT {URL_NODE_COLUMNS} FROM node WHERE node_id = $url_node LIMIT 2"
        ))
        .bind(("url_node", node_id.to_owned()))
        .await?
        .check()?;
    let nodes: Vec<serde_json::Value> = snapshot.take(0)?;
    let asset = captured_url_asset(&nodes, &petal)?.ok_or_else(|| {
        anyhow::anyhow!("This node has no placed model; a Portal URL cannot be stored on it")
    })?;
    let mut snapshot = db
        .query(format!(
            "SELECT {URL_MODEL_COLUMNS} FROM model WHERE asset_id = $url_asset LIMIT 2"
        ))
        .bind(("url_asset", asset.clone()))
        .await?
        .check()?;
    let models: Vec<serde_json::Value> = snapshot.take(0)?;
    let initialize = models.is_empty();
    if initialize {
        anyhow::ensure!(
            url.as_deref().is_some_and(|value| !value.is_empty()),
            "No local Portal metadata exists; clearing an absent URL makes no change"
        );
    } else {
        anyhow::ensure!(models.len() == 1 && models[0]["petal_id"].as_str() == Some(petal.as_str()),
            "Portal URL storage is unavailable: placed-model metadata is ambiguous or in another Petal");
        captured_model_url(&models[0])?;
    }
    let (mut statements, mut bindings) = access.into_parts();
    statements.extend([
        format!("IF (SELECT {URL_NODE_COLUMNS} FROM node WHERE node_id = $url_node LIMIT 2) != $url_nodes {{ THROW 'URL node changed'; }}"),
        format!("IF (SELECT {URL_MODEL_COLUMNS} FROM model WHERE asset_id = $url_asset LIMIT 2) != $url_models {{ THROW 'URL model changed'; }}"),
        "IF (SELECT VALUE node_id FROM node WHERE asset_id = $url_asset AND (tombstone = NONE OR tombstone = NULL) LIMIT 2) != [$url_node] { THROW 'URL model is shared; select an independent model'; }".into(),
    ]);
    bindings.extend([
        ("url_node".into(), json!(node_id)),
        ("url_nodes".into(), json!(nodes)),
        ("url_asset".into(), json!(asset)),
        ("url_models".into(), json!(models)),
        ("url_petal".into(), json!(petal)),
    ]);
    if initialize {
        statements.push("IF (SELECT VALUE asset_id FROM asset WHERE asset_id = $url_asset LIMIT 2) != [$url_asset] { THROW 'URL source asset is missing or ambiguous'; }".into());
    }
    statements.push(
        if initialize {
            "CREATE model CONTENT {asset_id: $url_asset, petal_id: $url_petal, transform: (SELECT position, elevation, rotation, scale FROM node WHERE node_id = $url_node)[0], external_url: $url_value, tags: []} RETURN AFTER"
        } else if url.is_some() {
            "UPDATE model SET external_url = $url_value WHERE asset_id = $url_asset RETURN AFTER"
        } else {
            "UPDATE model SET external_url = NONE WHERE asset_id = $url_asset RETURN AFTER"
        }.into(),
    );
    if let Some(url) = &url {
        bindings.push(("url_value".into(), json!(url)));
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_owned()),
        op_type: OpType::UpdateModelMeta,
        payload: json!({"asset_id":asset,"external_url":url,"portal_metadata_initialized":initialize}),
        sig: "00".repeat(64),
    };
    Ok(PreparedUrlWrite {
        statements,
        bindings,
        entry,
        url,
    })
}

/// Read current node/model URL under one captured Viewer fence.
pub(crate) async fn read_node_url_handler(
    db: &Db,
    node_id: &str,
    expected_scope: &str,
    expected_asset_id: Option<&str>,
    local_did: &str,
) -> anyhow::Result<fe_runtime::messages::NodeUrlSnapshot> {
    use serde_json::json;
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("URL target is unavailable"))?;
    anyhow::ensure!(scope == expected_scope, "URL target ancestry changed");
    let access = super::entity::prepare_scope_permission(
        db,
        &scope,
        local_did,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let petal = crate::parse_scope(&scope)?
        .petal_id
        .ok_or_else(|| anyhow::anyhow!("URL target has no Petal"))?;
    let mut captured = db
        .query(format!(
            "SELECT {URL_NODE_COLUMNS} FROM node WHERE node_id = $url_read_node LIMIT 2"
        ))
        .bind(("url_read_node", node_id.to_owned()))
        .await?
        .check()?;
    let nodes: Vec<serde_json::Value> = captured.take(0)?;
    let asset = captured_url_asset(&nodes, &petal)?;
    anyhow::ensure!(
        expected_asset_id.is_none_or(|expected| asset.as_deref() == Some(expected)),
        "URL node's model changed"
    );
    let (mut guards, mut bindings) = access.into_parts();
    guards.push(format!("IF (SELECT {URL_NODE_COLUMNS} FROM node WHERE node_id = $url_read_node LIMIT 2) != $url_read_nodes {{ THROW 'URL node identity changed'; }}"));
    bindings.extend([
        ("url_read_node".into(), json!(node_id)),
        ("url_read_nodes".into(), json!(nodes)),
        ("url_read_asset".into(), json!(asset)),
    ]);
    let model_index = guards.len() + 1;
    let mut script = String::from("BEGIN TRANSACTION;\n");
    for guard in guards {
        script.push_str(guard.trim().trim_end_matches(';'));
        script.push_str(";\n");
    }
    script.push_str(&format!("SELECT {URL_MODEL_COLUMNS} FROM model WHERE asset_id = $url_read_asset LIMIT 2;\nSELECT VALUE node_id FROM node WHERE asset_id = $url_read_asset AND (tombstone = NONE OR tombstone = NULL) LIMIT 2;\nCOMMIT TRANSACTION;"));
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut response = query
        .await
        .map_err(|_| anyhow::anyhow!("URL read transaction transport failed"))?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        return Err(url_read_transaction_error(errors, model_index));
    }
    let rows: Vec<serde_json::Value> = response
        .take(model_index)
        .map_err(|_| anyhow::anyhow!("URL model projection could not be decoded"))?;
    let references: Vec<String> = response
        .take(model_index + 1)
        .map_err(|_| anyhow::anyhow!("URL model references could not be decoded"))?;
    use fe_runtime::messages::NodeUrlStorage;
    let exclusive = references.len() == 1 && references[0] == node_id;
    let (url, storage) = if asset.is_none() {
        anyhow::ensure!(rows.is_empty(), "Unexpected assetless URL model");
        (None, NodeUrlStorage::NoAsset)
    } else if rows.is_empty() {
        (
            None,
            if exclusive {
                NodeUrlStorage::MissingModel
            } else {
                NodeUrlStorage::SharedModel
            },
        )
    } else {
        anyhow::ensure!(
            rows.len() == 1
                && rows[0]["asset_id"].as_str() == asset.as_deref()
                && rows[0]["petal_id"].as_str() == Some(petal.as_str()),
            "URL model identity is unavailable"
        );
        let storage = if exclusive {
            NodeUrlStorage::Ready
        } else {
            NodeUrlStorage::SharedModel
        };
        (captured_model_url(&rows[0])?, storage)
    };
    Ok(fe_runtime::messages::NodeUrlSnapshot {
        node_id: node_id.to_owned(),
        scope,
        asset_id: asset,
        url,
        storage,
    })
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
