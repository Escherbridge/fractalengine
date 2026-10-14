use tracing::instrument;

use fe_runtime::messages::DbResult;

use crate::op_log::commit_operation_atomic;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{replicate_row_with_petal, BlobStoreHandle, ReplicationSender};

/// Set or clear a petal's terrain config (None clears via `terrain = NONE`),
/// op-logging the write before it mutates (§log-first-commit, campaign plan
/// §3 S6 step 8 — closes the prior parity gap: this write used to neither
/// op-log nor replicate) and replicating the read-back row (the authoritative
/// echo) to the verse. Errors when no petal row matched.
#[instrument(skip(db, terrain, repl_tx, blob_store))]
pub(crate) async fn set_petal_terrain_handler(
    db: &Db,
    petal_id: &str,
    terrain: Option<&serde_json::Value>,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Option<serde_json::Value>> {
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(petal_id.to_string()),
        op_type: OpType::SetPetalTerrain,
        payload: serde_json::json!({ "petal_id": petal_id, "terrain": terrain }),
        sig: "00".repeat(64),
    };
    let (statement, bindings): (String, Vec<(String, serde_json::Value)>) = match terrain {
        Some(config) => (
            "UPDATE petal SET terrain = $config WHERE petal_id = $pid RETURN AFTER".to_string(),
            vec![
                ("config".to_string(), config.clone()),
                ("pid".to_string(), serde_json::json!(petal_id)),
            ],
        ),
        None => (
            "UPDATE petal SET terrain = NONE WHERE petal_id = $pid RETURN AFTER".to_string(),
            vec![("pid".to_string(), serde_json::json!(petal_id))],
        ),
    };
    let (_, mut statement_rows) =
        commit_operation_atomic(db, entry, std::slice::from_ref(&statement), bindings)
            .await
            .map_err(|e| anyhow::anyhow!("SetPetalTerrain failed: {e:#}"))?;
    let rows = statement_rows.pop().unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        anyhow::bail!("SetPetalTerrain: no petal row matched");
    };
    let stored = row.get("terrain").filter(|v| !v.is_null()).cloned();

    // Replicate the read-back row — best-effort: a verse/scope lookup failure
    // never fails the write itself (the DB mutation already committed).
    if let Ok(row_json) = serde_json::to_vec(&row) {
        if let Ok(Some(scope)) =
            crate::handlers::crud::resolve_petal_scope_handler(db, petal_id).await
        {
            if let Ok(parts) = crate::scope::parse_scope(&scope) {
                replicate_row_with_petal(
                    repl_tx,
                    blob_store,
                    &parts.verse_id,
                    "petal",
                    petal_id,
                    &row_json,
                    Some(petal_id.to_string()),
                );
            }
        }
    }
    Ok(stored)
}

/// The DB thread's reply to `SetPetalTerrain`: the stored terrain echoing
/// `correlation_id`; on failure a correlated `PetalTerrainSetFailed` for an
/// API set, the legacy wildcard `Error` for a GUI set (`None`).
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
pub async fn set_petal_terrain_reply(
    db: &Db,
    petal_id: String,
    terrain: Option<serde_json::Value>,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    correlation_id: Option<String>,
) -> DbResult {
    match set_petal_terrain_handler(db, &petal_id, terrain.as_ref(), repl_tx, blob_store).await {
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

    /// Full verse/fractal/petal chain so `resolve_petal_scope_handler`
    /// resolves a real `verse_id` for replication (critic step 8 tests).
    async fn db_with_full_hierarchy(petal_id: &str) -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        crate::op_log::init_hlc(0);
        db.query(
            "CREATE verse CONTENT { verse_id: 'v1', name: 'V', created_by: 'owner-did', \
             created_at: '2026-10-11T00:00:00Z', default_access: 'viewer' }",
        )
        .await
        .expect("seed verse")
        .check()
        .expect("seed verse");
        db.query(
            "CREATE fractal CONTENT { fractal_id: 'f1', verse_id: 'v1', owner_did: 'owner-did', \
             name: 'F', created_at: '2026-10-11T00:00:00Z' }",
        )
        .await
        .expect("seed fractal")
        .check()
        .expect("seed fractal");
        db.query(
            "CREATE petal CONTENT { petal_id: $pid, fractal_id: 'f1', name: 'P', \
             node_id: 'anchor-node', created_at: '2026-10-11T00:00:00Z' }",
        )
        .bind(("pid", petal_id.to_string()))
        .await
        .expect("seed petal")
        .check()
        .expect("seed petal");
        db
    }

    /// Critic step 8: `SetPetalTerrain` previously neither replicated nor
    /// op-logged. A real `repl_tx` capture + `ApplyReplicatedRow` on a second
    /// DB proves the gap is closed — the peer converges on the author's terrain.
    #[tokio::test]
    async fn set_petal_terrain_replicates_and_a_peer_converges_on_it() {
        use fe_runtime::blob_store::mock::MockBlobStore;

        let author_db = db_with_full_hierarchy("p1").await;
        let mock_store = std::sync::Arc::new(MockBlobStore::new());
        let blob_store: BlobStoreHandle = mock_store.clone();
        let (repl_tx, repl_rx) = crossbeam::channel::bounded(4);
        let terrain = serde_json::json!({ "tileset_hexon_uris": ["tileset-a"] });

        let stored = set_petal_terrain_handler(
            &author_db,
            "p1",
            Some(&terrain),
            Some(&repl_tx),
            &blob_store,
        )
        .await
        .expect("set ok");
        assert_eq!(stored, Some(terrain.clone()));

        let event = repl_rx
            .try_recv()
            .expect("a ReplicationEvent was emitted for the terrain write");
        assert_eq!(event.table, "petal");
        assert_eq!(event.record_id, "p1");
        let row_bytes = mock_store
            .bytes_for(&event.content_hash)
            .expect("blob present");

        let peer_db = db_with_full_hierarchy("p1").await;
        let outcome = crate::handlers::replicated_row::apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &row_bytes,
            "owner-did",
            None,
        )
        .await
        .expect("apply ok");
        assert!(matches!(
            outcome,
            fe_runtime::messages::ReplicatedRowOutcome::Applied
        ));
        let converged = get_petal_terrain_handler(&peer_db, "p1")
            .await
            .expect("get ok");
        assert_eq!(converged, Some(terrain));
    }

    /// Critic step 8: a shape-invalid `terrain` (not an object / a non-string
    /// URI / a `../` URI / oversize) is denied at the inbound gate, never
    /// applied — fe-database has no fe-terrain dependency, so this is a
    /// structural check only (`terrain_bridge`'s parse is the second line of
    /// defence).
    #[tokio::test]
    async fn an_inbound_petal_terrain_that_fails_the_shape_check_is_denied() {
        let bad_terrains = [
            serde_json::json!("not-an-object"),
            serde_json::json!({ "tileset_hexon_uris": [42] }),
            serde_json::json!({ "tileset_hexon_uris": ["../escape"] }),
            serde_json::json!({ "tileset_hexon_uris": ["ok"], "padding": "x".repeat(70_000) }),
        ];
        for bad in bad_terrains {
            let peer_db = db_with_full_hierarchy("p1").await;
            let row = serde_json::json!({
                "petal_id": "p1",
                "fractal_id": "f1",
                "name": "P",
                "node_id": "anchor-node",
                "created_at": "2026-10-11T00:00:00Z",
                "visibility": "private",
                "tags": [],
                "terrain": bad,
            });
            let row_bytes = serde_json::to_vec(&row).unwrap();
            let outcome = crate::handlers::replicated_row::apply_replicated_row_handler(
                &peer_db,
                "v1",
                "petal",
                "p1",
                &row_bytes,
                "owner-did",
                None,
            )
            .await
            .expect("apply ok");
            assert!(
                matches!(outcome, fe_runtime::messages::ReplicatedRowOutcome::Denied),
                "{bad:?} must be denied"
            );
        }
    }
}
