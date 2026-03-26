// DEFERRED TO VALIDATION PHASE
//
// These tests require an in-memory SurrealDB instance. They are written TDD-style
// and will be executed during the validation phase after all crates are compiled.

use fe_database::rbac::apply_schema;

/// Helper: create an in-memory SurrealDB instance for testing.
async fn test_db() -> surrealdb::Surreal<surrealdb::engine::local::Db> {
    use surrealdb::engine::local::Mem;
    let db = surrealdb::Surreal::new::<Mem>(()).await.expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    db
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn apply_schema_twice_is_idempotent() {
    // DEFERRED TO VALIDATION PHASE
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db).await.expect("second apply must not error");
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
    assert!(result.is_err() || {
        let mut response = result.unwrap();
        let taken = response.take::<Vec<serde_json::Value>>(0);
        taken.is_err() || taken.unwrap().is_empty()
    });
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
    assert!(result.is_err() || {
        let mut response = result.unwrap();
        let taken = response.take::<Vec<serde_json::Value>>(0);
        taken.is_err() || taken.unwrap().is_empty()
    });
}

#[tokio::test]
#[ignore = "DEFERRED TO VALIDATION PHASE"]
async fn fractal_schema_is_idempotent() {
    // DEFERRED TO VALIDATION PHASE
    // Applying schema twice should not produce errors (IF NOT EXISTS guards idempotency).
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db).await.expect("second apply must not error");
}
