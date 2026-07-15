//! READ-BACK tests for the iot_reading write path (FR-1,
//! iot_spatial_reporting_20260714) — see `src/AGENTS.md` §iot-readings.

use fe_database::handlers::iot_reading::{insert_readings, IotIngestError, IotReadingInput};

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

async fn setup_db() -> Db {
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db)
        .await
        .expect("apply schema");
    db
}

/// Seed an anchor node (geometry cast per `src/AGENTS.md` §geometry-inserts).
async fn seed_node(db: &Db, petal_id: &str, node_id: &str, x: f64, z: f64) {
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE node CONTENT { \
         node_id: $nid, petal_id: $pid, display_name: 'anchor', \
         position: <geometry<point>> [$x, $z], elevation: 0.0, \
         rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
         interactive: false, created_at: $now }",
    )
    .bind(("nid", node_id.to_string()))
    .bind(("pid", petal_id.to_string()))
    .bind(("x", x))
    .bind(("z", z))
    .bind(("now", now))
    .await
    .expect("seed node")
    .check()
    .expect("seed node check");
}

fn reading(node_id: &str, metric: &str, value: f64, recorded_at: Option<&str>) -> IotReadingInput {
    IotReadingInput {
        node_id: node_id.to_string(),
        metric: metric.to_string(),
        value,
        units: "C".to_string(),
        recorded_at: recorded_at.map(str::to_string),
    }
}

async fn select_all_readings(db: &Db) -> Vec<serde_json::Value> {
    let mut res = db
        .query("SELECT * FROM iot_reading ORDER BY recorded_at_ms ASC")
        .await
        .expect("select readings");
    res.take::<Vec<serde_json::Value>>(0).expect("rows")
}

#[tokio::test]
async fn batch_insert_reads_back_every_field() {
    let db = setup_db().await;
    seed_node(&db, "p1", "n1", 1.0, 2.0).await;
    seed_node(&db, "p1", "n2", 3.0, 4.0).await;

    let batch = vec![
        reading("n1", "temperature_c", 21.5, Some("2026-07-15T10:00:00Z")),
        reading("n1", "temperature_c", 22.0, Some("2026-07-15T11:00:00Z")),
        reading("n2", "humidity_pct", 55.25, Some("2026-07-15T10:30:00Z")),
    ];
    let n = insert_readings(&db, "p1", "did:key:z6MkSensor", &batch)
        .await
        .expect("insert batch");
    assert_eq!(n, 3);

    // READ-BACK: every persisted field verified.
    let rows = select_all_readings(&db).await;
    assert_eq!(rows.len(), 3);

    let first = &rows[0];
    assert_eq!(first["node_id"], "n1");
    assert_eq!(first["petal_id"], "p1");
    assert_eq!(first["metric"], "temperature_c");
    assert_eq!(first["value"].as_f64(), Some(21.5));
    assert_eq!(first["units"], "C");
    assert_eq!(first["source_did"], "did:key:z6MkSensor");
    assert!(first["reading_id"].as_str().is_some_and(|s| !s.is_empty()));

    // Sensor timestamp round-trips (normalized to UTC RFC-3339).
    let recorded = first["recorded_at"].as_str().expect("recorded_at");
    let parsed = chrono::DateTime::parse_from_rfc3339(recorded).expect("rfc3339");
    assert_eq!(
        parsed.timestamp_millis(),
        first["recorded_at_ms"].as_i64().expect("ms")
    );
    assert_eq!(
        first["recorded_at_ms"].as_i64(),
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-07-15T10:00:00Z")
                .expect("fixture ts")
                .timestamp_millis()
        )
    );

    // Ordering by recorded_at_ms puts the n2 10:30 row in the middle.
    assert_eq!(rows[1]["node_id"], "n2");
    assert_eq!(rows[2]["value"].as_f64(), Some(22.0));

    // HLC stamps are present and strictly ordered across the batch.
    let hlcs: Vec<i64> = rows
        .iter()
        .filter_map(|r| r["hlc_timestamp"].as_i64())
        .collect();
    assert_eq!(hlcs.len(), 3);
    assert!(hlcs.iter().all(|h| *h > 0));
}

#[tokio::test]
async fn unknown_anchor_rejects_whole_batch() {
    let db = setup_db().await;
    seed_node(&db, "p1", "n1", 0.0, 0.0).await;

    let batch = vec![
        reading("n1", "temperature_c", 20.0, None),
        reading("n-ghost", "temperature_c", 21.0, None),
    ];
    let err = insert_readings(&db, "p1", "did:key:z6MkSensor", &batch)
        .await
        .expect_err("ghost anchor must fail");
    assert!(
        matches!(err, IotIngestError::UnknownAnchor(ref id) if id == "n-ghost"),
        "{err}"
    );

    // All-or-nothing: nothing persisted, including the valid n1 row.
    assert!(select_all_readings(&db).await.is_empty());
}

#[tokio::test]
async fn anchor_in_other_petal_rejected() {
    let db = setup_db().await;
    seed_node(&db, "p-other", "n-foreign", 0.0, 0.0).await;

    let err = insert_readings(
        &db,
        "p1",
        "did:key:z6MkSensor",
        &[reading("n-foreign", "temperature_c", 20.0, None)],
    )
    .await
    .expect_err("foreign-petal anchor must fail");
    assert!(matches!(err, IotIngestError::UnknownAnchor(_)), "{err}");
    assert!(select_all_readings(&db).await.is_empty());
}

#[tokio::test]
async fn invalid_timestamp_rejected_before_any_insert() {
    let db = setup_db().await;
    seed_node(&db, "p1", "n1", 0.0, 0.0).await;

    let batch = vec![
        reading("n1", "temperature_c", 20.0, Some("2026-07-15T10:00:00Z")),
        reading("n1", "temperature_c", 21.0, Some("not-a-timestamp")),
    ];
    let err = insert_readings(&db, "p1", "did:key:z6MkSensor", &batch)
        .await
        .expect_err("bad rfc3339 must fail");
    assert!(matches!(err, IotIngestError::InvalidTimestamp(_)), "{err}");
    assert!(select_all_readings(&db).await.is_empty());
}

#[tokio::test]
async fn empty_metric_rejected() {
    let db = setup_db().await;
    seed_node(&db, "p1", "n1", 0.0, 0.0).await;

    let err = insert_readings(
        &db,
        "p1",
        "did:key:z6MkSensor",
        &[reading("n1", "  ", 1.0, None)],
    )
    .await
    .expect_err("blank metric must fail");
    assert!(matches!(err, IotIngestError::EmptyMetric), "{err}");
    assert!(select_all_readings(&db).await.is_empty());
}

#[tokio::test]
async fn empty_batch_is_ok_zero() {
    let db = setup_db().await;
    let n = insert_readings(&db, "p1", "did:key:z6MkSensor", &[])
        .await
        .expect("empty batch");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn missing_recorded_at_defaults_to_server_time() {
    let db = setup_db().await;
    seed_node(&db, "p1", "n1", 0.0, 0.0).await;

    let before_ms = chrono::Utc::now().timestamp_millis();
    insert_readings(
        &db,
        "p1",
        "did:key:z6MkSensor",
        &[reading("n1", "temperature_c", 20.0, None)],
    )
    .await
    .expect("insert");
    let after_ms = chrono::Utc::now().timestamp_millis();

    let rows = select_all_readings(&db).await;
    assert_eq!(rows.len(), 1);
    let ms = rows[0]["recorded_at_ms"].as_i64().expect("ms");
    assert!(
        (before_ms..=after_ms).contains(&ms),
        "server timestamp {ms} outside [{before_ms}, {after_ms}]"
    );
    assert!(rows[0]["recorded_at"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
}
