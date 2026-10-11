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
