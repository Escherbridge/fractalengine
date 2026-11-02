//! Bounded material-only reads for the active scene; see src/AGENTS.md.

use crate::repo::Db;
use fe_runtime::node_history::NodeAccessGrant;
use fe_runtime::scene_materials::{
    NodeMaterial, SceneMaterials, MATERIAL_BATCH_LIMIT, MATERIAL_JSON_BYTES,
};

async fn require_unique_id(db: &Db, table: &str, key: &str, id: &str) -> anyhow::Result<()> {
    let mut response = db
        .query(format!(
            "SELECT VALUE {key} FROM {table} WHERE {key} = $id LIMIT 2"
        ))
        .bind(("id", id.to_owned()))
        .await?
        .check()?;
    if response.take::<Vec<String>>(0)?.len() != 1 {
        anyhow::bail!("not found or not permitted");
    }
    Ok(())
}

pub(crate) async fn read(
    db: &Db,
    petal_id: &str,
    expected_scope: &str,
    node_ids: &[String],
    local_did: &str,
) -> anyhow::Result<SceneMaterials> {
    if node_ids.is_empty()
        || node_ids.len() > MATERIAL_BATCH_LIMIT
        || node_ids.iter().any(|id| id.is_empty() || id.len() > 128)
        || node_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != node_ids.len()
    {
        anyhow::bail!("invalid material projection batch");
    }
    let scope = super::crud::resolve_petal_scope_handler(db, petal_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    if scope != expected_scope {
        anyhow::bail!("not found or not permitted");
    }
    super::node_history::authorize_scope(
        db,
        &NodeAccessGrant::Local,
        local_did,
        &scope,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let parts = crate::parse_scope(&scope)?;
    let fractal = parts
        .fractal_id
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    for (table, key, id) in [
        ("verse", "verse_id", parts.verse_id.as_str()),
        ("fractal", "fractal_id", fractal.as_str()),
        ("petal", "petal_id", petal_id),
    ] {
        require_unique_id(db, table, key, id).await?;
    }
    let mut values = Vec::with_capacity(node_ids.len());
    for node_id in node_ids {
        if super::crud::resolve_node_scope_handler(db, node_id)
            .await?
            .as_deref()
            != Some(scope.as_str())
        {
            anyhow::bail!("not found or not permitted");
        }
        require_unique_id(db, "node", "node_id", node_id).await?;
        let mut response = db.query("SELECT node_id, (object::keys(properties ?? {}) CONTAINS 'material') AS has_material, (object::keys(properties ?? {}) CONTAINS 'primitive') AS has_primitive, properties.material AS material FROM node WHERE node_id = $node AND petal_id = $petal AND tombstone = NONE AND petal_id IN (SELECT VALUE petal_id FROM petal WHERE petal_id = $petal AND fractal_id = $fractal AND fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE fractal_id = $fractal AND verse_id = $verse)) LIMIT 2")
            .bind(("node", node_id.clone())).bind(("petal", petal_id.to_owned()))
            .bind(("fractal", fractal.clone())).bind(("verse", parts.verse_id.clone())).await?.check()?;
        let rows: Vec<serde_json::Value> = response.take(0)?;
        if rows.len() != 1
            || rows[0].get("node_id").and_then(|value| value.as_str()) != Some(node_id.as_str())
        {
            anyhow::bail!("not found or not permitted");
        }
        let material_json = if rows[0]
            .get("has_material")
            .and_then(|value| value.as_bool())
            == Some(true)
        {
            Some(serde_json::to_string(
                rows[0].get("material").unwrap_or(&serde_json::Value::Null),
            )?)
        } else {
            None
        };
        let truncated = material_json
            .as_ref()
            .is_some_and(|value| value.len() > MATERIAL_JSON_BYTES);
        values.push(NodeMaterial {
            node_id: node_id.clone(),
            material_json: material_json.filter(|_| !truncated),
            truncated,
            has_primitive: rows[0]
                .get("has_primitive")
                .and_then(|value| value.as_bool())
                == Some(true),
        });
    }
    if super::crud::resolve_petal_scope_handler(db, petal_id)
        .await?
        .as_deref()
        != Some(scope.as_str())
    {
        anyhow::bail!("not found or not permitted");
    }
    for node_id in node_ids {
        require_unique_id(db, "node", "node_id", node_id).await?;
        if super::crud::resolve_node_scope_handler(db, node_id)
            .await?
            .as_deref()
            != Some(scope.as_str())
        {
            anyhow::bail!("not found or not permitted");
        }
    }
    for (table, key, id) in [
        ("verse", "verse_id", parts.verse_id.as_str()),
        ("fractal", "fractal_id", fractal.as_str()),
        ("petal", "petal_id", petal_id),
    ] {
        require_unique_id(db, table, key, id).await?;
    }
    super::node_history::authorize_scope(
        db,
        &NodeAccessGrant::Local,
        local_did,
        &scope,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    Ok(SceneMaterials {
        petal_id: petal_id.to_owned(),
        scope,
        values,
    })
}
