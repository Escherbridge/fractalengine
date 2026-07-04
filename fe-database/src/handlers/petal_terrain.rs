use tracing::instrument;

use crate::repo::Db;

/// Set or clear a petal's terrain config (None clears via `terrain = NONE`).
#[instrument(skip(db, terrain))]
pub(crate) async fn set_petal_terrain_handler(
    db: &Db,
    petal_id: &str,
    terrain: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    match terrain {
        Some(config) => {
            db.query("UPDATE petal SET terrain = $config WHERE petal_id = $pid")
                .bind(("config", config.clone()))
                .bind(("pid", petal_id.to_string()))
                .await
                .map_err(|e| anyhow::anyhow!("SetPetalTerrain DB query failed: {e}"))?;
        }
        None => {
            db.query("UPDATE petal SET terrain = NONE WHERE petal_id = $pid")
                .bind(("pid", petal_id.to_string()))
                .await
                .map_err(|e| anyhow::anyhow!("SetPetalTerrain (clear) DB query failed: {e}"))?;
        }
    }
    Ok(())
}

/// Load a petal's terrain config; `None` when the petal has no terrain set.
#[instrument(skip(db))]
pub(crate) async fn get_petal_terrain_handler(
    db: &Db,
    petal_id: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let mut res = db
        .query("SELECT terrain FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("GetPetalTerrain DB query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("GetPetalTerrain take failed: {e}"))?;

    let terrain = rows.first().and_then(|r| r.get("terrain")).cloned();
    match terrain {
        Some(v) if !v.is_null() => Ok(Some(v)),
        _ => Ok(None),
    }
}
