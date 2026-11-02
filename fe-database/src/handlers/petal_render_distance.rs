//! Petal render-distance column (`petal.render_distance`) — p2p_asset_streaming
//! S6: a durable per-petal residency horizon that replicates, Editor+ gated,
//! op-logged before the row mutates. See `src/AGENTS.md` §handlers "Petal
//! render distance" and §log-first-commit.

use tracing::instrument;

use fe_runtime::messages::{CallerAuth, DbResult};

use crate::handlers::correlated_failure;
use crate::handlers::visibility_groups::authorize_edit;
use crate::op_log::commit_operation_atomic;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{replicate_row_with_petal, BlobStoreHandle, ReplicationSender};

/// Upper clamp — mirrors the schema `ASSERT` and `fe-ui`'s `RENDER_DISTANCE_MAX`
/// (`fe-ui/src/settings.rs`); the plan's "ceiling also binds the petal column"
/// invariant (campaign plan §3 S6 "Invariants").
pub const RENDER_DISTANCE_MAX: f64 = 50_000.0;

/// `None` means "clear" (inherit the global). `Some` outside `[0,
/// RENDER_DISTANCE_MAX]` or non-finite is refused before any write or op-log
/// entry — the strictest-reader standard this column's writer must meet
/// (RUNBOOK §5), applied here to the LOCAL write path too.
fn validate_render_distance(value: Option<f32>) -> Result<Option<f64>, String> {
    match value {
        None => Ok(None),
        Some(v) if v.is_finite() && v >= 0.0 && (v as f64) <= RENDER_DISTANCE_MAX => {
            Ok(Some(v as f64))
        }
        Some(v) => Err(format!(
            "render_distance {v} out of range [0, {RENDER_DISTANCE_MAX}]"
        )),
    }
}

/// Resolve the petal's scope → Editor+ (`authorize_edit`) → validate → op-log
/// the write (§log-first-commit) → `UPDATE … RETURN AFTER` → replicate the
/// read-back row (the echo is authoritative, house doctrine). Errors before
/// any DB mutation are returned as plain strings for `correlated_failure`.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(db, repl_tx, blob_store))]
pub(crate) async fn set_petal_render_distance_handler(
    db: &Db,
    local_did: &str,
    auth: &CallerAuth,
    petal_id: &str,
    render_distance: Option<f32>,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> Result<Option<f32>, String> {
    let scope = crate::handlers::crud::resolve_petal_scope_handler(db, petal_id)
        .await
        .map_err(|e| format!("SetPetalRenderDistance: scope lookup failed: {e}"))?
        .ok_or_else(|| format!("SetPetalRenderDistance: no petal {petal_id}"))?;
    authorize_edit(db, local_did, auth, scope.clone(), "SetPetalRenderDistance").await?;

    let value = validate_render_distance(render_distance)?;

    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(petal_id.to_string()),
        op_type: OpType::SetPetalRenderDistance,
        payload: serde_json::json!({ "petal_id": petal_id, "render_distance": value }),
        sig: "00".repeat(64),
    };
    let (statement, bindings): (String, Vec<(String, serde_json::Value)>) = match value {
        Some(v) => (
            "UPDATE petal SET render_distance = $rd WHERE petal_id = $pid RETURN AFTER".to_string(),
            vec![
                ("rd".to_string(), serde_json::json!(v)),
                ("pid".to_string(), serde_json::json!(petal_id)),
            ],
        ),
        None => (
            "UPDATE petal SET render_distance = NONE WHERE petal_id = $pid RETURN AFTER"
                .to_string(),
            vec![("pid".to_string(), serde_json::json!(petal_id))],
        ),
    };
    let (_, mut statement_rows) =
        commit_operation_atomic(db, entry, std::slice::from_ref(&statement), bindings)
            .await
            .map_err(|e| format!("SetPetalRenderDistance failed: {e:#}"))?;
    let rows = statement_rows.pop().unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Err(format!(
            "SetPetalRenderDistance: no petal row matched {petal_id}"
        ));
    };
    let stored = row
        .get("render_distance")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

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

/// Load a petal's `render_distance`; `None` means inherit the global.
#[instrument(skip(db))]
pub(crate) async fn get_petal_render_distance_handler(
    db: &Db,
    petal_id: &str,
) -> anyhow::Result<Option<f32>> {
    let mut res = db
        .query("SELECT render_distance FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("GetPetalRenderDistance DB query failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("GetPetalRenderDistance take failed: {e}"))?;
    Ok(rows
        .first()
        .and_then(|r| r.get("render_distance"))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32))
}

/// The DB thread's reply to both `SetPetalRenderDistance` and
/// `GetPetalRenderDistance` — one shared reply family (mirrors
/// `PetalVisibilityGroupsLoaded`), echoing `correlation_id`.
#[allow(clippy::too_many_arguments)]
pub async fn set_petal_render_distance_reply(
    db: &Db,
    local_did: &str,
    petal_id: String,
    render_distance: Option<f32>,
    auth: CallerAuth,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    correlation_id: Option<String>,
) -> DbResult {
    match set_petal_render_distance_handler(
        db,
        local_did,
        &auth,
        &petal_id,
        render_distance,
        repl_tx,
        blob_store,
    )
    .await
    {
        Ok(render_distance) => DbResult::PetalRenderDistanceLoaded {
            petal_id,
            render_distance,
            correlation_id,
        },
        Err(e) => correlated_failure(
            &correlation_id,
            format!("Set petal render distance failed: {e}"),
        ),
    }
}

/// The DB thread's reply to `GetPetalRenderDistance`.
pub async fn get_petal_render_distance_reply(
    db: &Db,
    petal_id: String,
    correlation_id: Option<String>,
) -> DbResult {
    match get_petal_render_distance_handler(db, &petal_id).await {
        Ok(render_distance) => DbResult::PetalRenderDistanceLoaded {
            petal_id,
            render_distance,
            correlation_id,
        },
        Err(e) => correlated_failure(
            &correlation_id,
            format!("Get petal render distance failed: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::replicated_row::apply_replicated_row_handler;
    use fe_runtime::blob_store::mock::MockBlobStore;

    async fn db_with_petal(petal_id: &str) -> Db {
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
        db.query("CREATE role CONTENT { scope: 'VERSE#v1', peer_did: 'owner-did', role: 'owner' }")
            .await
            .expect("seed role")
            .check()
            .expect("seed role");
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

    fn mem_blob_store() -> BlobStoreHandle {
        std::sync::Arc::new(MockBlobStore::new())
    }

    fn owner_auth() -> CallerAuth {
        CallerAuth::Identified {
            did: "owner-did".to_string(),
            role: "owner".to_string(),
        }
    }

    fn viewer_auth() -> CallerAuth {
        CallerAuth::Identified {
            did: "viewer-did".to_string(),
            role: "viewer".to_string(),
        }
    }

    #[tokio::test]
    async fn set_then_load_hierarchy_carries_the_value() {
        let db = db_with_petal("p1").await;
        let store = mem_blob_store();
        let got = set_petal_render_distance_handler(
            &db,
            "owner-did",
            &owner_auth(),
            "p1",
            Some(1234.5),
            None,
            &store,
        )
        .await
        .expect("set ok");
        assert_eq!(got, Some(1234.5));

        let loaded = get_petal_render_distance_handler(&db, "p1")
            .await
            .expect("get ok");
        assert_eq!(loaded, Some(1234.5));
        // Declarations persist independently of the residency column.
        use crate::handlers::petal_requirements as requirements;
        use fe_runtime::petal_requirements::{ManifestHexonEntry, PetalRequirements};
        let before = requirements::get(&db, "owner-did", "p1").await.unwrap();
        assert!(before.requirements.hexons.is_empty());
        let draft = PetalRequirements {
            hexons: vec![ManifestHexonEntry {
                hexon_id: "terrain-v1".into(),
                hexon_type: "TerrainTileset".into(),
                required: true,
            }],
        };
        let saved = requirements::set(&db, "owner-did", &before, &draft)
            .await
            .unwrap();
        assert_eq!(saved.requirements, draft);
        assert_eq!(
            requirements::get(&db, "owner-did", "p1").await.unwrap(),
            saved
        );
        let viewer = requirements::get(&db, "viewer-did", "p1").await.unwrap();
        assert!(!viewer.can_manage);
        let op_rows: Vec<serde_json::Value> = db
            .query("SELECT * FROM op_log ORDER BY lamport_clock")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        for actor in ["viewer-did", "editor-did"] {
            if actor == "editor-did" {
                db_seed_role(&db, actor, "editor").await;
            }
            assert!(
                requirements::set(&db, actor, &saved, &PetalRequirements::default())
                    .await
                    .is_err()
            );
        }
        assert!(
            requirements::set(&db, "owner-did", &before, &PetalRequirements::default())
                .await
                .is_err(),
            "stale raw document cannot overwrite a save"
        );
        let invalid = PetalRequirements {
            hexons: vec![draft.hexons[0].clone(), draft.hexons[0].clone()],
        };
        assert!(requirements::set(&db, "owner-did", &saved, &invalid)
            .await
            .is_err());
        let after_refused: Vec<serde_json::Value> = db
            .query("SELECT * FROM op_log ORDER BY lamport_clock")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(after_refused, op_rows, "denials add no intention");
        assert_eq!(
            requirements::get(&db, "owner-did", "p1").await.unwrap(),
            saved
        );
        db.query("DEFINE EVENT refuse_requirements ON TABLE petal WHEN $event = 'UPDATE' THEN { THROW 'induced requirements failure'; }").await.unwrap().check().unwrap();
        assert!(
            requirements::set(&db, "owner-did", &saved, &PetalRequirements::default())
                .await
                .is_err()
        );
        assert_eq!(
            requirements::get(&db, "owner-did", "p1").await.unwrap(),
            saved
        );
        let after_rollback: Vec<serde_json::Value> = db
            .query("SELECT * FROM op_log ORDER BY lamport_clock")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            after_rollback, op_rows,
            "induced storage failure rolls back intention and document"
        );
        db.query("REMOVE EVENT refuse_requirements ON TABLE petal")
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query("CREATE fractal CONTENT {fractal_id:'f2',verse_id:'v1',owner_did:'owner-did',name:'F2',created_at:'2026-11-02T00:00:00Z'}; UPDATE petal SET fractal_id='f2' WHERE petal_id='p1'").await.unwrap().check().unwrap();
        assert!(
            requirements::set(&db, "owner-did", &saved, &PetalRequirements::default())
                .await
                .is_err(),
            "old ancestry is not retargeted"
        );
        db.query("UPDATE petal SET fractal_id='f1',hexon_manifest=$legacy WHERE petal_id='p1'")
            .bind((
                "legacy",
                serde_json::json!({"hexons":draft.hexons,"render_distance":500.0,"fallback":"sign"}),
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let legacy = requirements::get(&db, "owner-did", "p1").await.unwrap();
        let legacy_saved = requirements::set(&db, "owner-did", &legacy, &PetalRequirements::default())
            .await
            .unwrap();
        assert_eq!(
            legacy_saved.document.as_ref().unwrap()["render_distance"],
            500.0
        );
        assert_eq!(legacy_saved.document.as_ref().unwrap()["fallback"], "sign");
        assert_eq!(
            get_petal_render_distance_handler(&db, "p1").await.unwrap(),
            Some(1234.5)
        );
        db.query("UPDATE petal SET hexon_manifest={unknown:'hidden',hexons:[]} WHERE petal_id='p1'")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            requirements::get(&db, "owner-did", "p1").await.is_err(),
            "unknown fields refuse lossy editing"
        );
    }
    #[tokio::test]
    async fn clearing_restores_inherit() {
        let db = db_with_petal("p1").await;
        let store = mem_blob_store();
        set_petal_render_distance_handler(
            &db,
            "owner-did",
            &owner_auth(),
            "p1",
            Some(900.0),
            None,
            &store,
        )
        .await
        .expect("set ok");
        let cleared = set_petal_render_distance_handler(
            &db,
            "owner-did",
            &owner_auth(),
            "p1",
            None,
            None,
            &store,
        )
        .await
        .expect("clear ok");
        assert_eq!(cleared, None);
        assert_eq!(
            get_petal_render_distance_handler(&db, "p1")
                .await
                .expect("get ok"),
            None
        );
    }

    #[tokio::test]
    async fn a_viewer_cannot_set_it() {
        let db = db_with_petal("p1").await;
        let store = mem_blob_store();
        let err = set_petal_render_distance_handler(
            &db,
            "viewer-did",
            &viewer_auth(),
            "p1",
            Some(500.0),
            None,
            &store,
        )
        .await
        .expect_err("viewer must be denied");
        assert!(!err.is_empty());
        // The row must be untouched.
        assert_eq!(
            get_petal_render_distance_handler(&db, "p1")
                .await
                .expect("get ok"),
            None
        );
    }

    #[tokio::test]
    async fn out_of_range_values_are_refused() {
        let db = db_with_petal("p1").await;
        let store = mem_blob_store();
        for bad in [f32::NAN, -1.0, 50_001.0] {
            let err = set_petal_render_distance_handler(
                &db,
                "owner-did",
                &owner_auth(),
                "p1",
                Some(bad),
                None,
                &store,
            )
            .await
            .expect_err(&format!("{bad} must be refused"));
            assert!(!err.is_empty());
        }
        assert_eq!(
            get_petal_render_distance_handler(&db, "p1")
                .await
                .expect("get ok"),
            None,
            "no bad value ever lands"
        );
    }

    #[tokio::test]
    async fn an_editor_authored_replicated_petal_row_applies_and_a_viewer_authored_one_is_denied() {
        // Author-side DB: an Editor sets render_distance, producing the row
        // bytes a peer would receive.
        let author_db = db_with_petal("p1").await;
        db_seed_role(&author_db, "editor-did", "editor").await;
        let store = mem_blob_store();
        set_petal_render_distance_handler(
            &author_db,
            "editor-did",
            &CallerAuth::Identified {
                did: "editor-did".to_string(),
                role: "editor".to_string(),
            },
            "p1",
            Some(777.0),
            None,
            &store,
        )
        .await
        .expect("editor set ok");
        let row_bytes = fetch_row_bytes(&author_db, "p1").await;

        // Peer DB: the same author is Editor there too — applies.
        let peer_db = db_with_petal("p1").await;
        db_seed_role(&peer_db, "editor-did", "editor").await;
        let outcome = apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &row_bytes,
            "editor-did",
            None,
        )
        .await
        .expect("apply ok");
        assert!(matches!(
            outcome,
            fe_runtime::messages::ReplicatedRowOutcome::Applied
        ));
        assert_eq!(
            get_petal_render_distance_handler(&peer_db, "p1")
                .await
                .expect("get ok"),
            Some(777.0)
        );

        use crate::handlers::petal_requirements as requirements;
        use fe_runtime::petal_requirements::{ManifestHexonEntry, PetalRequirements};
        let empty = requirements::get(&author_db, "owner-did", "p1")
            .await
            .unwrap();
        let draft = PetalRequirements {
            hexons: vec![ManifestHexonEntry {
                hexon_id: "model-v2".into(),
                hexon_type: "Model".into(),
                required: false,
            }],
        };
        requirements::set(&author_db, "owner-did", &empty, &draft)
            .await
            .unwrap();
        let declaration_bytes = fetch_row_bytes(&author_db, "p1").await;
        let denied = apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &declaration_bytes,
            "editor-did",
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(
                denied,
                fe_runtime::messages::ReplicatedRowOutcome::Deferred(_)
            ),
            "an Editor cannot change requirements via a whole-row publication"
        );
        let accepted = apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &declaration_bytes,
            "owner-did",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            accepted,
            fe_runtime::messages::ReplicatedRowOutcome::Applied
        );
        assert_eq!(
            requirements::get(&peer_db, "owner-did", "p1")
                .await
                .unwrap()
                .requirements,
            draft
        );
        let unchanged = apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &declaration_bytes,
            "editor-did",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            unchanged,
            fe_runtime::messages::ReplicatedRowOutcome::Applied,
            "Editor terrain/distance updates may carry unchanged requirements"
        );
        let mut malformed: serde_json::Value = serde_json::from_slice(&declaration_bytes).unwrap();
        malformed["hexon_manifest"] = serde_json::json!({"hexons":[],"hidden":"must not drop"});
        let malformed = serde_json::to_vec(&malformed).unwrap();
        assert_eq!(
            apply_replicated_row_handler(&peer_db, "v1", "petal", "p1", &malformed, "owner-did", None)
                .await
                .unwrap(),
            fe_runtime::messages::ReplicatedRowOutcome::Denied
        );

        // Peer DB where the author has no granted role (viewer by default) —
        // the whole row is deferred (RP-3: retried once the role lands), never applied.
        let denying_peer_db = db_with_petal("p1").await;
        let outcome = apply_replicated_row_handler(
            &denying_peer_db,
            "v1",
            "petal",
            "p1",
            &row_bytes,
            "editor-did",
            None,
        )
        .await
        .expect("apply ok");
        assert!(matches!(
            outcome,
            fe_runtime::messages::ReplicatedRowOutcome::Deferred(
                fe_runtime::messages::DeferralReason::AuthorRole { .. }
            )
        ));
        assert_eq!(
            get_petal_render_distance_handler(&denying_peer_db, "p1")
                .await
                .expect("get ok"),
            None,
            "a deferred row never touches the peer's petal"
        );
    }
    #[tokio::test]
    async fn an_inbound_out_of_range_value_is_denied() {
        let peer_db = db_with_petal("p1").await;
        db_seed_role(&peer_db, "editor-did", "editor").await;
        let row_bytes = serde_json::to_vec(&serde_json::json!({
            "petal_id": "p1",
            "fractal_id": "f1",
            "name": "P",
            "node_id": "anchor-node",
            "created_at": "2026-10-11T00:00:00Z",
            "visibility": "private",
            "tags": [],
            "render_distance": 99_999.0,
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &peer_db,
            "v1",
            "petal",
            "p1",
            &row_bytes,
            "editor-did",
            None,
        )
        .await
        .expect("apply ok");
        assert!(matches!(
            outcome,
            fe_runtime::messages::ReplicatedRowOutcome::Denied
        ));
    }

    #[tokio::test]
    async fn petal_settings_writes_land_in_the_op_log_before_the_row() {
        let db = db_with_petal("p1").await;
        let store = mem_blob_store();
        set_petal_render_distance_handler(
            &db,
            "owner-did",
            &owner_auth(),
            "p1",
            Some(321.0),
            None,
            &store,
        )
        .await
        .expect("set ok");
        let mut res = db
            .query("SELECT count() FROM op_log WHERE op_type = 'SetPetalRenderDistance' GROUP ALL")
            .await
            .expect("op_log query");
        let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
        let count = rows
            .first()
            .and_then(|r| r.get("count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(count, 1, "exactly one op-log row per write");
    }

    async fn db_seed_role(db: &Db, did: &str, role: &str) {
        db.query("CREATE role CONTENT { scope: 'VERSE#v1', peer_did: $did, role: $role }")
            .bind(("did", did.to_string()))
            .bind(("role", role.to_string()))
            .await
            .expect("seed role")
            .check()
            .expect("seed role");
    }

    async fn fetch_row_bytes(db: &Db, petal_id: &str) -> Vec<u8> {
        let mut res = db
            .query("SELECT * FROM petal WHERE petal_id = $pid LIMIT 1")
            .bind(("pid", petal_id.to_string()))
            .await
            .expect("fetch")
            .check()
            .expect("fetch");
        let rows: Vec<serde_json::Value> = res.take(0).expect("take");
        serde_json::to_vec(&rows[0]).expect("serialize")
    }
}
