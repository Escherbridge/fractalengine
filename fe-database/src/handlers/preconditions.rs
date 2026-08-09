//! Cheap target checks required before a log-first mutation is appended.

use crate::repo::Db;

/// Require a node target to exist before its mutation enters the operation log.
pub(crate) async fn require_node_exists(
    db: &Db,
    node_id: &str,
    operation: &str,
) -> anyhow::Result<()> {
    require_record_exists(db, "node", "node_id", node_id, operation, "node").await
}

/// Require a petal and every ancestor in its canonical scope chain to exist.
pub(crate) async fn require_petal_scope(
    db: &Db,
    petal_id: &str,
    operation: &str,
) -> anyhow::Result<String> {
    crate::handlers::crud::resolve_petal_scope_handler(db, petal_id)
        .await
        .map_err(|error| anyhow::anyhow!("{operation} scope precondition failed: {error}"))?
        .ok_or_else(|| anyhow::anyhow!("{operation} matched no petal with petal_id = {petal_id}"))
}

/// Require a referenced asset to exist before a model-placement operation.
pub(crate) async fn require_asset_exists(
    db: &Db,
    asset_id: &str,
    operation: &str,
) -> anyhow::Result<()> {
    require_record_exists(db, "asset", "asset_id", asset_id, operation, "asset").await
}

/// Require every component of a canonical hierarchical scope to exist.
pub(crate) async fn require_scope_exists(
    db: &Db,
    scope: &str,
    operation: &str,
) -> anyhow::Result<()> {
    let parts = crate::parse_scope(scope)
        .map_err(|error| anyhow::anyhow!("{operation} invalid scope: {error}"))?;
    require_record_exists(db, "verse", "verse_id", &parts.verse_id, operation, "verse").await?;

    let Some(fractal_id) = parts.fractal_id else {
        return Ok(());
    };
    let actual_verse_id = require_parent_id(
        db,
        "fractal",
        "fractal_id",
        &fractal_id,
        "verse_id",
        operation,
        "fractal",
    )
    .await?;
    if actual_verse_id != parts.verse_id {
        anyhow::bail!(
            "{operation} fractal {fractal_id} is outside verse {}",
            parts.verse_id
        );
    }

    let Some(petal_id) = parts.petal_id else {
        return Ok(());
    };
    let actual_fractal_id = require_parent_id(
        db,
        "petal",
        "petal_id",
        &petal_id,
        "fractal_id",
        operation,
        "petal",
    )
    .await?;
    if actual_fractal_id != fractal_id {
        anyhow::bail!("{operation} petal {petal_id} is outside fractal {fractal_id}");
    }
    Ok(())
}

/// Require a room target to exist and belong to the authorized petal.
pub(crate) async fn require_room_in_petal(
    db: &Db,
    room_id: &str,
    petal_id: &str,
    operation: &str,
) -> anyhow::Result<String> {
    require_record_in_petal(db, "room", room_id, petal_id, operation, "room").await
}

/// Require a model target to exist and belong to the authorized petal.
pub(crate) async fn require_model_in_petal(
    db: &Db,
    model_id: &str,
    petal_id: &str,
    operation: &str,
) -> anyhow::Result<String> {
    require_record_in_petal(db, "model", model_id, petal_id, operation, "model").await
}

async fn require_record_exists(
    db: &Db,
    table: &str,
    id_field: &str,
    id: &str,
    operation: &str,
    target_kind: &str,
) -> anyhow::Result<()> {
    let sql = format!("SELECT {id_field} FROM {table} WHERE {id_field} = $id LIMIT 1");
    let mut result = db
        .query(sql)
        .bind(("id", id.to_string()))
        .await
        .map_err(|error| anyhow::anyhow!("{operation} precondition query failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("{operation} precondition statement failed: {error}"))?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|error| anyhow::anyhow!("{operation} precondition result read failed: {error}"))?;
    if rows.is_empty() {
        anyhow::bail!("{operation} matched no {target_kind} with {id_field} = {id}");
    }
    Ok(())
}

async fn require_record_in_petal(
    db: &Db,
    table: &str,
    id: &str,
    petal_id: &str,
    operation: &str,
    target_kind: &str,
) -> anyhow::Result<String> {
    let sql = format!("SELECT petal_id FROM {table} WHERE id = $id LIMIT 1");
    let mut result = db
        .query(sql)
        .bind(("id", id.to_string()))
        .await
        .map_err(|error| anyhow::anyhow!("{operation} precondition query failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("{operation} precondition statement failed: {error}"))?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|error| anyhow::anyhow!("{operation} precondition result read failed: {error}"))?;
    let Some(actual_petal_id) = rows
        .first()
        .and_then(|row| row.get("petal_id"))
        .and_then(serde_json::Value::as_str)
    else {
        anyhow::bail!("{operation} matched no {target_kind} with id = {id}");
    };
    if actual_petal_id != petal_id {
        anyhow::bail!("{operation} {target_kind} with id = {id} is outside petal {petal_id}");
    }
    Ok(actual_petal_id.to_string())
}

async fn require_parent_id(
    db: &Db,
    table: &str,
    id_field: &str,
    id: &str,
    parent_field: &str,
    operation: &str,
    target_kind: &str,
) -> anyhow::Result<String> {
    let sql = format!("SELECT {parent_field} FROM {table} WHERE {id_field} = $id LIMIT 1");
    let mut result = db
        .query(sql)
        .bind(("id", id.to_string()))
        .await
        .map_err(|error| anyhow::anyhow!("{operation} precondition query failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("{operation} precondition statement failed: {error}"))?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|error| anyhow::anyhow!("{operation} precondition result read failed: {error}"))?;
    rows.first()
        .and_then(|row| row.get(parent_field))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!("{operation} matched no {target_kind} with {id_field} = {id}")
        })
}
