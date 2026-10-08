//! READ-BACK tests for the iot_reading write path (FR-1,
//! iot_spatial_reporting_20260714) — see `src/AGENTS.md` §iot-readings.

use fe_database::handlers::iot_reading::{insert_readings, IotIngestError, IotReadingInput};

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

async fn setup_db() -> Db {
    // The write handler packs HLC timestamps; production init happens during
    // DB startup, which this raw in-memory setup bypasses.
    fe_database::op_log::init_hlc(0);
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

// ---------------------------------------------------------------------------
// A11 — the replication emission seam
// ---------------------------------------------------------------------------

/// A11: `insert_readings_with_replication` emits exactly one
/// `ReplicationEvent` per accepted row, each carrying the verse **and** the
/// petal, keyed by that row's own `reading_id`, and the emitted bytes are the
/// row as persisted.
#[tokio::test]
async fn replication_seam_emits_one_event_per_row_with_verse_and_petal() {
    use std::sync::Arc;

    use fe_database::handlers::iot_reading::insert_readings_with_replication;
    use fe_database::BlobStoreHandle;

    let db = setup_db().await;
    seed_node(&db, "petal-1", "node-a", 0.0, 0.0).await;
    let mock = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
    let store: BlobStoreHandle = mock.clone();
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);

    let n = insert_readings_with_replication(
        &db,
        "petal-1",
        Some("verse-1"),
        "did:key:z6MkSensor",
        &[
            reading("node-a", "temperature_c", 21.5, None),
            reading("node-a", "humidity_pct", 55.0, None),
        ],
        Some(&store),
        Some(&repl_tx),
    )
    .await
    .expect("insert with replication");

    assert_eq!(n, 2, "both rows are durably accepted");
    assert_eq!(select_all_readings(&db).await.len(), 2);

    let mut events = Vec::new();
    while let Ok(evt) = repl_rx.try_recv() {
        events.push(evt);
    }
    assert_eq!(events.len(), 2, "one ReplicationEvent per accepted row");

    let persisted = select_all_readings(&db).await;
    for evt in &events {
        assert_eq!(evt.verse_id, "verse-1", "the event names its verse replica");
        assert_eq!(evt.table, "iot_reading");
        assert_eq!(evt.petal_id.as_deref(), Some("petal-1"));
        // The doc key is the union key, and the blob holds exactly the row that
        // was persisted (so a peer can apply it byte-for-byte).
        let row = persisted
            .iter()
            .find(|r| r["reading_id"].as_str() == Some(evt.record_id.as_str()))
            .expect("emitted reading_id must be a persisted row");
        let bytes = mock.bytes_for(&evt.content_hash).expect("blob present");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["value"],
            row["value"],
            "published bytes match the durable row"
        );
    }
    assert_ne!(
        events[0].record_id, events[1].record_id,
        "each row gets its own reading_id key"
    );
}

/// A11: without a verse (the delegating-shim path) nothing is published —
/// the rows are still written, so a caller that has no replication seam loses
/// nothing durable.
#[tokio::test]
async fn replication_seam_without_verse_emits_nothing() {
    use std::sync::Arc;

    use fe_database::handlers::iot_reading::insert_readings_with_replication;
    use fe_database::BlobStoreHandle;

    let db = setup_db().await;
    seed_node(&db, "petal-1", "node-a", 0.0, 0.0).await;
    let store: BlobStoreHandle = Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);

    let n = insert_readings_with_replication(
        &db,
        "petal-1",
        None,
        "did:key:z6MkSensor",
        &[reading("node-a", "temperature_c", 21.5, None)],
        Some(&store),
        Some(&repl_tx),
    )
    .await
    .expect("insert without verse context");

    assert_eq!(n, 1);
    assert_eq!(select_all_readings(&db).await.len(), 1);
    assert!(
        repl_rx.try_recv().is_err(),
        "no verse context → nothing published"
    );
}
