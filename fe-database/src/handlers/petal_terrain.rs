use tracing::instrument;

use fe_runtime::messages::DbResult;

use crate::repo::Db;

/// Set or clear a petal's terrain config (None clears via `terrain = NONE`)
/// and return the value read back from the written row (the authoritative
/// echo). Errors when no petal row matched.
#[instrument(skip(db, terrain))]
pub(crate) async fn set_petal_terrain_handler(
    db: &Db,
    petal_id: &str,
    terrain: Option<&serde_json::Value>,
) -> anyhow::Result<Option<serde_json::Value>> {
    let query = match terrain {
        Some(config) => db
            .query("UPDATE petal SET terrain = $config WHERE petal_id = $pid RETURN terrain")
            .bind(("config", config.clone()))
            .bind(("pid", petal_id.to_string())),
        None => db
            .query("UPDATE petal SET terrain = NONE WHERE petal_id = $pid RETURN terrain")
            .bind(("pid", petal_id.to_string())),
    };
    let mut res = query
        .await
        .map_err(|e| anyhow::anyhow!("SetPetalTerrain DB query failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("SetPetalTerrain DB update failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("SetPetalTerrain read-back failed: {e}"))?;
    let Some(row) = rows.first() else {
        anyhow::bail!("SetPetalTerrain: no petal row matched");
    };
    Ok(row.get("terrain").filter(|v| !v.is_null()).cloned())
}

/// The DB thread's reply to `SetPetalTerrain`: the stored terrain echoing
/// `correlation_id`; on failure a correlated `PetalTerrainSetFailed` for an
/// API set, the legacy wildcard `Error` for a GUI set (`None`).
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
pub async fn set_petal_terrain_reply(
    db: &Db,
    petal_id: String,
    terrain: Option<serde_json::Value>,
    correlation_id: Option<String>,
) -> DbResult {
    match set_petal_terrain_handler(db, &petal_id, terrain.as_ref()).await {
        Ok(terrain) => DbResult::PetalTerrainLoaded {
            petal_id,
            terrain,
            correlation_id,
        },
        Err(e) => match correlation_id {
            Some(correlation_id) => DbResult::PetalTerrainSetFailed {
                petal_id,
                error: format!("Set petal terrain failed: {e}"),
                correlation_id,
            },
            None => DbResult::Error(format!("Set petal terrain failed: {e}")),
        },
    }
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

/// The DB thread's reply to `GetPetalTerrain`: a correlated (API) read gets
/// the private `PetalTerrainFetched` / `RequestFailed`; a GUI load (`None`)
/// keeps the legacy `PetalTerrainLoaded` / wildcard `Error`.
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
pub async fn get_petal_terrain_reply(
    db: &Db,
    petal_id: String,
    correlation_id: Option<String>,
) -> DbResult {
    match (
        get_petal_terrain_handler(db, &petal_id).await,
        correlation_id,
    ) {
        (Ok(terrain), Some(correlation_id)) => DbResult::PetalTerrainFetched {
            petal_id,
            terrain,
            correlation_id,
        },
        (Ok(terrain), None) => DbResult::PetalTerrainLoaded {
            petal_id,
            terrain,
            correlation_id: None,
        },
        (Err(e), correlation_id) => {
            super::correlated_failure(&correlation_id, format!("Get petal terrain failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db_with_petal(petal_id: &str, terrain: serde_json::Value) -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db.query(
            "CREATE petal CONTENT { petal_id: $pid, fractal_id: 'f1', name: 'P', \
             node_id: 'anchor-node', created_at: '2026-10-11T00:00:00Z', terrain: $terrain }",
        )
        .bind(("pid", petal_id.to_string()))
        .bind(("terrain", terrain))
        .await
        .expect("seed petal")
        .check()
        .expect("seed petal");
        db
    }

    /// FEAPI-GETTERRAIN-CORR: an API (correlated) read gets the PRIVATE
    /// `PetalTerrainFetched` echoing its id — never `PetalTerrainLoaded`,
    /// which the GUI terrain arms would re-apply to the viewport — while a
    /// GUI load keeps `PetalTerrainLoaded`.
    #[tokio::test]
    async fn correlated_get_is_private_and_gui_get_keeps_the_loaded_echo() {
        let terrain = serde_json::json!({ "tileset_hexon_uris": ["tileset-a"] });
        let db = db_with_petal("p1", terrain.clone()).await;

        match get_petal_terrain_reply(&db, "p1".to_string(), Some("api-1".to_string())).await {
            DbResult::PetalTerrainFetched {
                petal_id,
                terrain: Some(stored),
                correlation_id,
            } => {
                assert_eq!(
                    (petal_id.as_str(), correlation_id.as_str()),
                    ("p1", "api-1")
                );
                assert_eq!(stored["tileset_hexon_uris"], terrain["tileset_hexon_uris"]);
            }
            other => panic!("API read must be PetalTerrainFetched: {other:?}"),
        }
        match get_petal_terrain_reply(&db, "p1".to_string(), None).await {
            DbResult::PetalTerrainLoaded {
                petal_id,
                terrain: Some(_),
                correlation_id: None,
            } => assert_eq!(petal_id, "p1"),
            other => panic!("GUI load must keep PetalTerrainLoaded: {other:?}"),
        }
    }
}
