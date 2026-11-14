use tracing::instrument;

use fe_runtime::messages::DbResult;
use fe_runtime::terrain_grant::TerrainWriteGrant;

use crate::handlers::visibility_groups::authorize_edit;
use crate::op_log::commit_operation_atomic;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{BlobStoreHandle, ReplicationSender};

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
    grant: &TerrainWriteGrant,
    petal_id: &str,
    terrain: Option<&serde_json::Value>,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Option<serde_json::Value>> {
    let prepared = prepare_terrain_write(db, local_did, grant, petal_id, terrain).await?;
    let scope = prepared.scope.clone();
    let row = prepared.commit(db).await?;
    let stored = row.get("terrain").filter(|value| !value.is_null()).cloned();

    // Replicate the read-back row — best-effort: a verse/scope lookup failure
    // never fails the write itself (the DB mutation already committed).
    if let Ok(parts) = crate::scope::parse_scope(&scope) {
        if let Err(error) = crate::field_clock::publish_clocked_row(
            db, blob_store, repl_tx, local_did, &parts.verse_id, "petal", petal_id, true,
        )
        .await
        {
            tracing::warn!(petal_id, "committed petal terrain clocked publish failed: {error:#}");
        }
    }
    Ok(stored)
}

struct PreparedTerrainWrite {
    scope: String,
    petal: String,
    terrain: Option<serde_json::Value>,
    authority: super::entity::ParentMutation,
    credential_fence: Option<crate::GuardedStatements>,
}

/// Capture the actual local writer's authority and exact target ancestry.
async fn prepare_terrain_write(
    db: &Db,
    local_did: &str,
    grant: &TerrainWriteGrant,
    petal: &str,
    terrain: Option<&serde_json::Value>,
) -> anyhow::Result<PreparedTerrainWrite> {
    let scope = super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("SetPetalTerrain: no petal {petal}"))?;
    authorize_edit(db, local_did, &fe_runtime::messages::CallerAuth::Local, scope.clone(), "SetPetalTerrain")
        .await
        .map_err(anyhow::Error::msg)?;
    let credential_fence = match grant {
        TerrainWriteGrant::Local => None,
        TerrainWriteGrant::Api(api) => {
            use fe_policy::{Policy, TokenScopePolicy, AuthContext, Action, Scope};
            let admitted = crate::parse_scope(api.admitted_scope())?;
            anyhow::ensure!(api.petal() == petal && admitted.petal_id.as_deref() == Some(petal)
                && crate::build_scope(&admitted.verse_id, admitted.fractal_id.as_deref(), admitted.petal_id.as_deref()) == api.admitted_scope()
                && scope == api.admitted_scope() && api.role().is_at_least(fe_policy::RoleLevel::Editor),
                "Terrain admitted ancestry or role changed");
            let token = crate::parse_scope(api.token_scope())?;
            anyhow::ensure!(crate::build_scope(&token.verse_id, token.fractal_id.as_deref(), token.petal_id.as_deref()) == api.token_scope(), "Terrain token scope invalid");
            let context = AuthContext::ApiToken { subject: api.issuer().into(),
                scopes: vec![api.token_scope().into()], role: api.role() };
            anyhow::ensure!(TokenScopePolicy.evaluate(&context, &Action::Write, &Scope::new(&scope)).is_allow(),
                "Terrain token scope refused");
            let snapshot = crate::api_auth::terrain_snapshot(db, api.issuer(), api.jti()).await?;
            Some(crate::api_auth::terrain_credential_fence(api, &snapshot)?)
        }
    };
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
        credential_fence,
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
        if let Some((guards, values)) = self.credential_fence {
            statements.extend(guards);
            bindings.extend(values);
        }
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
    grant: TerrainWriteGrant,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    correlation_id: Option<String>,
) -> DbResult {
    match set_petal_terrain_handler(
        db,
        local_did,
        &grant,
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
        // DB-thread startup always defines api_token before serving (lib.rs startup).
        crate::api_token_store::apply_api_token_schema(&db).await.expect("api_token schema");
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
            &TerrainWriteGrant::Local,
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
                &TerrainWriteGrant::Local,
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
            &TerrainWriteGrant::Local,
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
        assert_api_terrain_guard_journey().await;
        use fe_runtime::blob_store::mock::MockBlobStore;

        let db = db_with_full_hierarchy("p1").await;
        let blob_store: BlobStoreHandle = std::sync::Arc::new(MockBlobStore::new());
        let (repl_tx, repl_rx) = crossbeam::channel::bounded(4);
        let terrain = serde_json::json!({ "tileset_hexon_uris": ["tileset-a"] });
        let before = op_log_count(&db).await;

        // No role row: the local node resolves to the verse default, `viewer`.
        {
            let auth = TerrainWriteGrant::Local;
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
            prepare_terrain_write(&db, "editor-did", &TerrainWriteGrant::Local, "p1", Some(&terrain))
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
            prepare_terrain_write(&db, "viewer-did", &TerrainWriteGrant::Local, "p1", Some(&terrain))
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
            prepare_terrain_write(&db, "owner-did", &TerrainWriteGrant::Local, "p1", Some(&terrain))
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
            &TerrainWriteGrant::Local,
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

    async fn assert_api_terrain_guard_journey() {
        use fe_runtime::terrain_grant::{TerrainApiGrant, TerrainWriteGrant};
        for case in ["valid", "revoke", "reset", "insert", "delete", "table-loss", "move-before", "move-after", "expired",
            "delete-before", "revoke-before", "malformed-generation", "old-generation", "new-generation", "legacy", "legacy-after-reset"] {
            let db = db_with_full_hierarchy("p1").await;
            let key = fe_identity::NodeKeypair::generate();
            let issuer = key.to_did_key();
            if case == "new-generation" {
                db.query("UPDATE api_auth_state:current SET generation=$next").bind(("next", ulid::Ulid::new().to_string())).await.unwrap().check().unwrap();
            }
            let generation = crate::api_auth::current_generation(&db).await.unwrap();
            let jti = if matches!(case, "legacy" | "legacy-after-reset") { ulid::Ulid::new().to_string() }
                else { crate::api_auth::new_jti(&generation).unwrap() };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let expiry = if case == "expired" { now.saturating_sub(1) } else { now + 3600 };
            let scope = "VERSE#v1-FRACTAL#f1-PETAL#p1";
            let token = fe_identity::api_token::mint_api_token_at(&key, "VERSE#v1", "editor", now.saturating_sub(2), expiry, &jti).unwrap();
            let claims = fe_identity::api_token::verify_api_token(&token, &key.verifying_key()).unwrap();
            let insert_token = "CREATE api_token CONTENT {jti:$jti,scope:'VERSE#v1',max_role:'editor',sub:$sub,created_at:'2026-11-04T00:00:00Z',expires_at:'2099-01-01T00:00:00Z',revoked:false}";
            if matches!(case, "revoke" | "delete" | "delete-before" | "revoke-before") {
                db.query(insert_token).bind(("jti", jti.clone())).bind(("sub", issuer.clone())).await.unwrap().check().unwrap();
            }
            let snapshot = crate::api_auth::terrain_snapshot(&db, &issuer, &jti).await.unwrap();
            if case == "valid" {
                let viewer = fe_identity::api_token::mint_api_token(&key, "VERSE#v1", "viewer", 3600, &jti).unwrap();
                let viewer = fe_identity::api_token::verify_api_token(&viewer, &key.verifying_key()).unwrap();
                assert!(TerrainApiGrant::verified(&viewer, &key.verifying_key(), "p1".into(), scope.into(), snapshot.clone()).is_err(), "a signed Viewer cannot become an API terrain grant");
            }
            let grant = TerrainWriteGrant::Api(TerrainApiGrant::verified(&claims, &key.verifying_key(), "p1".into(), scope.into(), snapshot).unwrap());
            let before = op_log_count(&db).await;
            let terrain = serde_json::json!({"tileset_hexon_uris":["terrain-guard"]});
            if matches!(case, "delete-before" | "revoke-before" | "malformed-generation" | "old-generation" | "legacy-after-reset") {
                match case {
                    "delete-before" => { db.query("DELETE api_token WHERE jti=$jti").bind(("jti", jti.clone())).await.unwrap().check().unwrap(); }
                    "revoke-before" => { db.query("UPDATE api_token SET revoked=true WHERE jti=$jti").bind(("jti", jti.clone())).await.unwrap().check().unwrap(); }
                    "malformed-generation" => { db.query("UPDATE api_auth_state:current SET generation='invalid'").await.unwrap().check().unwrap(); }
                    _ => { db.query("UPDATE api_auth_state:current SET generation=$next").bind(("next", ulid::Ulid::new().to_string())).await.unwrap().check().unwrap(); }
                }
                assert!(prepare_terrain_write(&db, "owner-did", &grant, "p1", Some(&terrain)).await.is_err(), "{case}");
                assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
                assert_eq!(op_log_count(&db).await, before);
                continue;
            }
            if case == "move-before" {
                db.query("CREATE fractal CONTENT {fractal_id:'f2',verse_id:'v1',owner_did:'owner-did',name:'F2',created_at:'2026-11-04T00:00:00Z'}; UPDATE petal SET fractal_id='f2' WHERE petal_id='p1'").await.unwrap().check().unwrap();
                assert!(prepare_terrain_write(&db, "owner-did", &grant, "p1", Some(&terrain)).await.is_err());
                assert_eq!(op_log_count(&db).await, before);
                assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None);
                continue;
            }
            let prepared = prepare_terrain_write(&db, "owner-did", &grant, "p1", Some(&terrain)).await.unwrap();
            match case {
                "revoke" => { db.query("UPDATE api_token SET revoked=true WHERE jti=$jti").bind(("jti", jti.clone())).await.unwrap().check().unwrap(); }
                "reset" => { db.query("UPDATE api_auth_state:current SET generation=$generation").bind(("generation", ulid::Ulid::new().to_string())).await.unwrap().check().unwrap(); }
                "insert" => { db.query(insert_token).bind(("jti", jti.clone())).bind(("sub", issuer.clone())).await.unwrap().check().unwrap(); }
                "delete" => { db.query("DELETE api_token WHERE jti=$jti").bind(("jti", jti.clone())).await.unwrap().check().unwrap(); }
                "table-loss" => { db.query("REMOVE TABLE api_token").await.unwrap().check().unwrap(); }
                "move-after" => { db.query("CREATE fractal CONTENT {fractal_id:'f2',verse_id:'v1',owner_did:'owner-did',name:'F2',created_at:'2026-11-04T00:00:00Z'}; UPDATE petal SET fractal_id='f2' WHERE petal_id='p1'").await.unwrap().check().unwrap(); }
                _ => {}
            }
            let result = prepared.commit(&db).await;
            if matches!(case, "valid" | "new-generation" | "legacy") {
                assert_eq!(result.unwrap()["terrain"], terrain);
                assert_eq!(op_log_count(&db).await, before + 1);
            } else {
                assert!(result.is_err(), "{case} must refuse the complete operation/row pair");
                assert_eq!(get_petal_terrain_handler(&db, "p1").await.unwrap(), None, "{case}");
                assert_eq!(op_log_count(&db).await, before, "{case}: no orphan operation");
            }
        }
    }
}
