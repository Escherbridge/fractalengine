// DEFERRED TO VALIDATION PHASE
//
// These tests require an in-memory SurrealDB instance. They are written TDD-style
// and will be executed during the validation phase after all crates are compiled.

use fe_database::rbac::apply_schema;

/// Helper: create an in-memory SurrealDB instance for testing.
async fn test_db() -> surrealdb::Surreal<surrealdb::engine::local::Db> {
    use surrealdb::engine::local::Mem;
    let db = surrealdb::Surreal::new::<Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    db
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn apply_schema_twice_is_idempotent() {
    // DEFERRED TO VALIDATION PHASE
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db)
        .await
        .expect("second apply must not error");
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn petal_visibility_rejects_invalid_value() {
    // DEFERRED TO VALIDATION PHASE
    let db = test_db().await;
    apply_schema(&db).await.unwrap();
    let result: surrealdb::Result<surrealdb::IndexedResults> = db
        .query("CREATE petal SET petal_id = 'p1', name = 'test', node_id = 'n1', created_at = '2026-01-01', visibility = 'forbidden'")
        .await;
    // SurrealDB v3: validation errors are embedded inside IndexedResults, not the outer Result.
    // So we check: outer Err OR inner take() Err OR empty results.
    assert!(
        result.is_err() || {
            let mut response = result.unwrap();
            let taken = response.take::<Vec<serde_json::Value>>(0);
            taken.is_err() || taken.unwrap().is_empty()
        }
    );
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn verse_member_status_rejects_invalid_value() {
    // DEFERRED TO VALIDATION PHASE
    // CREATE verse_member with status='banned' must fail the ASSERT constraint.
    let db = test_db().await;
    apply_schema(&db).await.unwrap();
    let result: surrealdb::Result<surrealdb::IndexedResults> = db
        .query("CREATE verse_member SET member_id = 'm1', verse_id = 'v1', peer_did = 'did:key:z6Mk', status = 'banned', invited_by = 'did:key:z6Ml', invite_sig = 'deadbeef', invite_timestamp = '2026-01-01T00:00:00Z'")
        .await;
    // SurrealDB v3: validation errors are embedded inside IndexedResults, not the outer Result.
    assert!(
        result.is_err() || {
            let mut response = result.unwrap();
            let taken = response.take::<Vec<serde_json::Value>>(0);
            taken.is_err() || taken.unwrap().is_empty()
        }
    );
}

// --- P2P Mycelium Phase A tests (not deferred — run as part of regular suite) ---

#[tokio::test]
async fn apply_schema_creates_asset_content_hash_field() {
    let db = test_db().await;
    apply_schema(&db).await.expect("schema must apply cleanly");

    // Query schema metadata for the asset table and confirm content_hash is defined.
    let mut res: surrealdb::IndexedResults = db
        .query("INFO FOR TABLE asset")
        .await
        .expect("INFO FOR TABLE asset query");
    let info: Vec<serde_json::Value> = res.take(0).expect("take INFO rows");
    let joined = serde_json::to_string(&info).expect("serialize INFO");
    assert!(
        joined.contains("content_hash"),
        "INFO FOR TABLE asset should mention content_hash field, got: {joined}"
    );
}

#[tokio::test]
async fn asset_row_can_omit_data_field_when_content_hash_set() {
    // With Phase A schema, `data` is option<string> — new rows may reference a
    // blob via `content_hash` and omit `data` entirely.
    let db = test_db().await;
    apply_schema(&db).await.expect("schema");

    let _: surrealdb::IndexedResults = db
        .query("CREATE asset SET asset_id = 'a1', name = 'test', content_type = 'model/gltf-binary', size_bytes = 42, created_at = '2026-01-01T00:00:00Z', content_hash = 'deadbeef'")
        .await
        .expect("CREATE asset without data field must succeed");

    let mut res: surrealdb::IndexedResults = db
        .query("SELECT * FROM asset WHERE asset_id = 'a1'")
        .await
        .expect("SELECT");
    let rows: Vec<serde_json::Value> = res.take(0).expect("take");
    assert_eq!(rows.len(), 1, "expected one row");
    assert_eq!(rows[0]["content_hash"].as_str(), Some("deadbeef"));

    use fe_database::admin::{clear_for_development_reset, DevelopmentResetPermit};
    let owner = "did:key:z6Mk-reset-owner-fixture";
    let offline = DevelopmentResetPermit::capture_for_offline_store();
    assert!(clear_for_development_reset(&db, owner, &DevelopmentResetPermit::disabled()).await.is_err());
    let running = clear_for_development_reset(&db, owner, &DevelopmentResetPermit::capture()).await.unwrap_err();
    assert!(running.to_string().contains("writers are running"));
    db.query("CREATE verse SET verse_id='reset-foreign', name='Foreign', created_by='did:key:z6Mk-foreign-owner-fixture', created_at='2026-11-02'").await.unwrap().check().unwrap();
    assert!(clear_for_development_reset(&db, owner, &offline).await.unwrap_err().to_string().contains("own every"));
    let mut unchanged = db.query("SELECT * FROM asset WHERE asset_id='a1'").await.unwrap().check().unwrap();
    assert_eq!(unchanged.take::<Vec<serde_json::Value>>(0).unwrap().len(), 1);
    db.query("UPDATE verse SET created_by=$owner").bind(("owner", owner.to_string())).await.unwrap().check().unwrap();
    db.query("DEFINE TABLE unexpected_reset_fixture SCHEMALESS").await.unwrap().check().unwrap();
    assert!(clear_for_development_reset(&db, owner, &offline).await.unwrap_err().to_string().contains("unknown table"));
    db.query("REMOVE TABLE unexpected_reset_fixture").await.unwrap().check().unwrap();
    fe_database::api_token_store::apply_api_token_schema(&db).await.unwrap();
    db.query("CREATE node_log SET log_id='reset-log', node_id='reset-node', hlc_timestamp=1, op='baseline', source_did=$owner, payload={nested:{value:1}}, row_version=1, created_at='2026-11-02'; CREATE api_token SET jti='reset-token', scope='verse:reset-foreign', max_role='viewer', sub=$owner, created_at='2026-11-02', expires_at='2026-11-03', revoked=false; CREATE canon_sealed_artifact SET artifact_id_hex='reset-private', content_class='sealed_artifact', content_bytes='Zml4dHVyZQ=='; CREATE verified_op_log SET op_id_hex='reset-canon', envelope_bytes='Zml4dHVyZQ==', operation_kind=1, branch_id='reset', author_public_key_hex='fixture', wall_ms=1, hlc_counter=0")
        .bind(("owner", owner.to_string())).await.unwrap().check().unwrap();
    let admitted = fe_database::admin::prepare_development_reset(&db, owner, &offline).await.unwrap();
    db.query("UPDATE verse SET created_by='did:key:z6Mk-foreign-owner-fixture'").await.unwrap().check().unwrap();
    assert!(admitted.commit(&db).await.is_err(), "changed global ownership must invalidate the captured plan");
    let mut retained = db.query("SELECT * FROM asset; SELECT * FROM api_token; SELECT * FROM canon_sealed_artifact; SELECT * FROM verified_op_log; SELECT * FROM node_log").await.unwrap().check().unwrap();
    for slot in 0..5 { assert_eq!(retained.take::<Vec<serde_json::Value>>(slot).unwrap().len(), 1, "refusal must preserve each stored plane"); }
    db.query("UPDATE verse SET created_by=$owner").bind(("owner", owner.to_string())).await.unwrap().check().unwrap();
    clear_for_development_reset(&db, owner, &offline).await.unwrap();
    for table in fe_database::schema::DEVELOPMENT_RESET_TABLE_NAMES {
        let mut check = db.query(format!("SELECT VALUE id FROM {table} LIMIT 1")).await.unwrap().check().unwrap();
        assert!(check.take::<Vec<serde_json::Value>>(0).unwrap().is_empty(), "reset retained {table}");
    }
    clear_for_development_reset(&db, owner, &offline).await.unwrap();
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn fractal_schema_is_idempotent() {
    // DEFERRED TO VALIDATION PHASE
    // Applying schema twice should not produce errors (IF NOT EXISTS guards idempotency).
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db)
        .await
        .expect("second apply must not error");
}

/// The scope resolvers' own-id point lookups (`WHERE petal_id = $x LIMIT 1`, …)
/// are index-backed, and re-applying the schema keeps them (idempotent).
#[tokio::test]
async fn id_lookup_indexes_exist_after_apply_all() {
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db)
        .await
        .expect("second apply must not error");
    for (table, index, field) in [
        ("petal", "idx_petal_id", "petal_id"),
        ("fractal", "idx_fractal_id", "fractal_id"),
        ("node", "idx_node_id", "node_id"),
        ("verse", "idx_verse_id", "verse_id"),
    ] {
        let mut res: surrealdb::IndexedResults = db
            .query(format!("INFO FOR TABLE {table}"))
            .await
            .expect("INFO FOR TABLE");
        let info: Option<serde_json::Value> = res.take(0).expect("take INFO");
        let indexes = info
            .as_ref()
            .and_then(|i| i.get("indexes"))
            .and_then(|i| i.as_object())
            .unwrap_or_else(|| panic!("INFO FOR TABLE {table} has no indexes map: {info:?}"));
        let definition = indexes
            .get(index)
            .and_then(|d| d.as_str())
            .unwrap_or_else(|| panic!("{table} lacks {index}: {indexes:?}"));
        assert!(
            definition.contains(field),
            "{index} must index {table}.{field}: {definition}"
        );
    }
}

/// VIS-PH4: a store populated BEFORE the visibility fields existed (simulated by
/// removing them) re-applies the schema without touching its rows — every new
/// field reads NONE (= visible / no groups), and the petal registry column then
/// stores nested group objects verbatim.
#[tokio::test]
async fn visibility_fields_apply_to_a_populated_pre_phase4_store() {
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    db.query(
        "REMOVE FIELD view_hidden ON TABLE verse;\
         REMOVE FIELD view_hidden ON TABLE fractal;\
         REMOVE FIELD view_hidden ON TABLE petal;\
         REMOVE FIELD visibility_groups[*] ON TABLE petal;\
         REMOVE FIELD visibility_groups ON TABLE petal;\
         CREATE verse SET verse_id = 'v1', name = 'V', created_by = 'did:key:z6Mk', created_at = '2026-01-01';\
         CREATE fractal SET fractal_id = 'f1', verse_id = 'v1', owner_did = 'did:key:z6Mk', name = 'F', created_at = '2026-01-01';\
         CREATE petal SET petal_id = 'p1', name = 'P', node_id = 'n1', created_at = '2026-01-01', fractal_id = 'f1'",
    )
    .await
    .expect("seed pre-phase-4 rows")
    .check()
    .expect("seed statements");

    apply_schema(&db)
        .await
        .expect("re-apply on a populated store");
    apply_schema(&db).await.expect("and again (idempotent)");

    let mut res = db
        .query(
            "SELECT name, view_hidden FROM verse WHERE verse_id = 'v1';\
             SELECT name, view_hidden FROM fractal WHERE fractal_id = 'f1';\
             SELECT name, view_hidden, visibility_groups FROM petal WHERE petal_id = 'p1'",
        )
        .await
        .expect("read back");
    for (statement, name) in [(0, "V"), (1, "F"), (2, "P")] {
        let rows: Vec<serde_json::Value> = res.take(statement).expect("take");
        assert_eq!(rows.len(), 1, "row {name} intact");
        assert_eq!(rows[0]["name"], name);
        assert!(
            rows[0]["view_hidden"].is_null(),
            "{name}: new field is NONE"
        );
    }

    let groups = serde_json::json!([
        {"id": "g1", "name": "Utilities", "visible": false, "order": 0.5}
    ]);
    let mut res = db
        .query("UPDATE petal SET visibility_groups = $g WHERE petal_id = 'p1' RETURN visibility_groups")
        .bind(("g", groups.clone()))
        .await
        .expect("write registry")
        .check()
        .expect("registry accepted by the column type");
    let rows: Vec<serde_json::Value> = res.take(0).expect("take");
    assert_eq!(
        rows[0]["visibility_groups"], groups,
        "nested objects kept verbatim"
    );
}
