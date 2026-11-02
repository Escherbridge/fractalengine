use tracing::instrument;

use fe_runtime::messages::{CallerAuth, DbResult};

use crate::handlers::visibility_groups::authorize_edit;
use crate::op_log::commit_operation_atomic;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{replicate_row_with_petal, BlobStoreHandle, ReplicationSender};

/// Set or clear a petal's terrain config (None clears via `terrain = NONE`),
/// op-logging the write before it mutates (§log-first-commit, campaign plan
/// §3 S6 step 8 — closes the prior parity gap: this write used to neither
/// op-log nor replicate) and replicating the read-back row (the authoritative
/// echo) to the verse. Editor+ at the petal scope comes first (RP-4 step 7,
/// the shape of `set_petal_render_distance_handler`): a write every peer would
/// defer is refused locally instead of silently diverging. Errors when no
/// petal row matched.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(db, terrain, repl_tx, blob_store))]
pub(crate) async fn set_petal_terrain_handler(
    db: &Db,
    local_did: &str,
    auth: &CallerAuth,
    petal_id: &str,
    terrain: Option<&serde_json::Value>,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Option<serde_json::Value>> {
    let prepared = prepare_terrain_write(db, local_did, auth, petal_id, terrain).await?;
    let scope = prepared.scope.clone();
    let row = prepared.commit(db).await?;
    let stored = row.get("terrain").filter(|value| !value.is_null()).cloned();

    // Replicate the read-back row — best-effort: a verse/scope lookup failure
    // never fails the write itself (the DB mutation already committed).
    if let Ok(row_json) = serde_json::to_vec(&row) {
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
    Ok(stored)
}

struct PreparedTerrainWrite {
    scope: String,
    petal: String,
    terrain: Option<serde_json::Value>,
    authority: super::entity::ParentMutation,
}

/// Capture the actual local writer's authority and exact target ancestry.
async fn prepare_terrain_write(
    db: &Db,
    local_did: &str,
    auth: &CallerAuth,
    petal: &str,
    terrain: Option<&serde_json::Value>,
) -> anyhow::Result<PreparedTerrainWrite> {
    let scope = super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("SetPetalTerrain: no petal {petal}"))?;
    authorize_edit(db, local_did, auth, scope.clone(), "SetPetalTerrain")
        .await
        .map_err(anyhow::Error::msg)?;
    let authority = super::entity::prepare_scope_permission(
        db,
        &scope,
        local_did,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    if let Some(config) = terrain {
        anyhow::ensure!(
            super::replicated_row::is_valid_inbound_petal_terrain(config),
            "SetPetalTerrain: terrain fails the shared shape check (tileset ids, cache_dir, size)"
        );
    }
    Ok(PreparedTerrainWrite {
        scope,
        petal: petal.into(),
        terrain: terrain.cloned(),
        authority,
    })
}

impl PreparedTerrainWrite {
    /// Fence authority, append the ordinary operation and read back its mutation atomically.
    async fn commit(self, db: &Db) -> anyhow::Result<serde_json::Value> {
        let entry = OpLogEntry {
            lamport_clock: 0,
            hlc_timestamp: String::new(),
            node_id: NodeId(self.petal.clone()),
            op_type: OpType::SetPetalTerrain,
            payload: serde_json::json!({"petal_id":self.petal,"terrain":self.terrain}),
            sig: "00".repeat(64),
        };
        let (mut statements, mut bindings) = self.authority.into_parts();
        bindings.push(("pid".into(), serde_json::json!(self.petal)));
        statements.push(match self.terrain {
            Some(config) => {
                bindings.push(("config".into(), config));
                "UPDATE petal SET terrain=$config WHERE petal_id=$pid RETURN AFTER".into()
            }
            None => "UPDATE petal SET terrain=NONE WHERE petal_id=$pid RETURN AFTER".into(),
        });
        let (_, mut rows) = commit_operation_atomic(db, entry, &statements, bindings)
            .await
            .map_err(|error| anyhow::anyhow!("SetPetalTerrain failed: {error:#}"))?;
        // The final caller slot is UPDATE; BEGIN and op-log slots are removed by the helper.
        let rows = rows.pop().unwrap_or_default();
        anyhow::ensure!(
            rows.len() == 1,
            "SetPetalTerrain committed but readback is unavailable; inspect before retrying"
        );
        Ok(rows.into_iter().next().unwrap())
    }
}

/// The DB thread's reply to `SetPetalTerrain`: the stored terrain echoing
/// `correlation_id`; on failure a correlated `PetalTerrainSetFailed` for an
/// API set, the legacy wildcard `Error` for a GUI set (`None`).
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
#[allow(clippy::too_many_arguments)]
pub async fn set_petal_terrain_reply(
    db: &Db,
    local_did: &str,
    petal_id: String,
    terrain: Option<serde_json::Value>,
    auth: CallerAuth,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    correlation_id: Option<String>,
) -> DbResult {
    match set_petal_terrain_handler(
        db,
        local_did,
        &auth,
        &petal_id,
        terrain.as_ref(),
        repl_tx,
        blob_store,
    )
    .await
    {
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
            "owner-did",
            &CallerAuth::Local,
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
            // S8b: the strictest reader's other untrusted fields.
            serde_json::json!({ "tileset_hexon_uris": ["CON"] }),
            serde_json::json!({ "cache_dir": "../../outside" }),
            serde_json::json!({ "cache_dir": "C:/Windows/System32" }),
            serde_json::json!({ "cache_dir": "/etc" }),
            serde_json::json!({ "elevation_api_key_env": "AWS_SECRET_ACCESS_KEY" }),
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

    /// S8b: the LOCAL writer satisfies the strictest reader — a terrain every
    /// peer would deny (here: one choosing which env var a node reads a
    /// secret from) is refused before the op-log, the row, or replication.
    #[tokio::test]
    async fn a_local_terrain_write_peers_would_deny_is_refused() {
        use fe_runtime::blob_store::mock::MockBlobStore;

        let db = db_with_full_hierarchy("p1").await;
        let blob_store: BlobStoreHandle = std::sync::Arc::new(MockBlobStore::new());
        let (repl_tx, repl_rx) = crossbeam::channel::bounded(4);
        for bad in [
            serde_json::json!({ "elevation_api_key_env": "AWS_SECRET_ACCESS_KEY" }),
            serde_json::json!({ "cache_dir": "../../outside" }),
        ] {
            let err = set_petal_terrain_handler(
                &db,
                "owner-did",
                &CallerAuth::Local,
                "p1",
                Some(&bad),
                Some(&repl_tx),
                &blob_store,
            )
            .await
            .expect_err("refused");
            assert!(err.to_string().contains("shape check"), "{err:#}");
        }
        assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
        assert!(repl_rx.try_recv().is_err(), "nothing was replicated");

        let good =
            serde_json::json!({ "cache_dir": "terrain_cache", "tileset_hexon_uris": ["ts-1"] });
        let stored = set_petal_terrain_handler(
            &db,
            "owner-did",
            &CallerAuth::Local,
            "p1",
            Some(&good),
            Some(&repl_tx),
            &blob_store,
        )
        .await
        .expect("a well-formed terrain still writes");
        assert_eq!(stored, Some(good));
    }

    async fn op_log_count(db: &Db) -> i64 {
        let rows: Vec<serde_json::Value> = db
            .query("SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .expect("count op_log")
            .take(0)
            .expect("count rows");
        rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0)
    }

    /// RP-4 step 7 (AC-21(d)): a Viewer's local `SetPetalTerrain` (the GUI's
    /// `Local` and an API token alike) is refused by `authorize_edit` before the
    /// shape check, the op-log entry, the row write and any replication send,
    /// so a local edit can never diverge from what every peer accepts.
    #[tokio::test]
    async fn a_viewers_local_set_petal_terrain_is_refused_and_nothing_is_written() {
        use fe_runtime::blob_store::mock::MockBlobStore;

        let db = db_with_full_hierarchy("p1").await;
        let blob_store: BlobStoreHandle = std::sync::Arc::new(MockBlobStore::new());
        let (repl_tx, repl_rx) = crossbeam::channel::bounded(4);
        let terrain = serde_json::json!({ "tileset_hexon_uris": ["tileset-a"] });
        let before = op_log_count(&db).await;

        // No role row: the local node resolves to the verse default, `viewer`.
        let viewer_token = CallerAuth::Identified {
            did: "api".to_string(),
            role: "viewer".to_string(),
        };
        for auth in [CallerAuth::Local, viewer_token] {
            let err = set_petal_terrain_handler(
                &db,
                "viewer-did",
                &auth,
                "p1",
                Some(&terrain),
                Some(&repl_tx),
                &blob_store,
            )
            .await
            .expect_err("a Viewer may not set the petal terrain");
            assert!(
                err.to_string().contains("SetPetalTerrain denied"),
                "{err:#}"
            );
        }
        assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
        assert_eq!(op_log_count(&db).await, before, "no op-log entry");
        assert!(repl_rx.try_recv().is_err(), "nothing was replicated");

        db.query("CREATE role CONTENT {scope:'VERSE#v1-FRACTAL#f1-PETAL#p1',peer_did:'editor-did',role:'editor'}").await.unwrap().check().unwrap();
        let prepared =
            prepare_terrain_write(&db, "editor-did", &CallerAuth::Local, "p1", Some(&terrain))
                .await
                .unwrap();
        db.query("DELETE role WHERE peer_did='editor-did'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let error = prepared
            .commit(&db)
            .await
            .expect_err("revocation after capture must refuse the same atomic mutation");
        assert!(format!("{error:#}").contains("authority changed"));
        assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
        assert_eq!(
            op_log_count(&db).await,
            before,
            "authority fence failure rolls back the ordinary operation"
        );

        db.query("UPDATE verse SET default_access='editor' WHERE verse_id='v1'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let prepared =
            prepare_terrain_write(&db, "viewer-did", &CallerAuth::Local, "p1", Some(&terrain))
                .await
                .unwrap();
        db.query("UPDATE verse SET default_access='viewer' WHERE verse_id='v1'")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            prepared.commit(&db).await.is_err(),
            "captured default access is fenced as well as explicit roles"
        );
        assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
        assert_eq!(op_log_count(&db).await, before);

        let prepared =
            prepare_terrain_write(&db, "owner-did", &CallerAuth::Local, "p1", Some(&terrain))
                .await
                .unwrap();
        db.query("CREATE fractal CONTENT {fractal_id:'f2',verse_id:'v1',owner_did:'owner-did',name:'F2',created_at:'2026-10-11T00:00:00Z'}; UPDATE petal SET fractal_id='f2' WHERE petal_id='p1'").await.unwrap().check().unwrap();
        let error = prepared
            .commit(&db)
            .await
            .expect_err("reparent after capture must refuse the same atomic mutation");
        assert!(format!("{error:#}").contains("ancestry changed"));
        assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
        assert_eq!(op_log_count(&db).await, before);
        assert!(
            repl_rx.try_recv().is_err(),
            "no refused plan can publish a terrain row"
        );
        db.query("UPDATE petal SET fractal_id='f1' WHERE petal_id='p1'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let readback = set_petal_terrain_handler(
            &db,
            "owner-did",
            &CallerAuth::Local,
            "p1",
            Some(&terrain),
            Some(&repl_tx),
            &blob_store,
        )
        .await
        .unwrap();
        assert_eq!(readback, Some(terrain));
        assert_eq!(op_log_count(&db).await, before + 1);
        assert!(
            repl_rx.try_recv().is_ok(),
            "only the successful committed readback is published"
        );
    }
}
