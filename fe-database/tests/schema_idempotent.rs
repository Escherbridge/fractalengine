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
async fn apply_schema_twice_is_idempotent() {
    // DEFERRED TO VALIDATION PHASE
    let db = test_db().await;
    apply_schema(&db).await.expect("first apply");
    apply_schema(&db).await.expect("second apply must not error");
}

#[tokio::test]
async fn petal_visibility_rejects_invalid_value() {
    // DEFERRED TO VALIDATION PHASE
    let db = test_db().await;
    apply_schema(&db).await.unwrap();
    let result: surrealdb::Result<surrealdb::Response> = db
        .query("CREATE petal SET visibility = 'forbidden'")
        .await;
    assert!(result.is_err() || {
        result
            .unwrap()
            .take::<Vec<serde_json::Value>>(0)
            .unwrap()
            .is_empty()
    });
}
