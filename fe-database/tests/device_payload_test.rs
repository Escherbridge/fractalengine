//! R5 device format descriptor + device payload flow tests (CDL-SPEC10 A3) —
//! through the public handlers and `DbCommand`-shaped inputs, like
//! `tests/iot_reading_test.rs`. See `src/AGENTS.md` §device-payload.

use std::sync::Arc;

use fe_database::handlers::device_payload::{
    insert_device_payloads, list_device_formats, list_device_payloads, register_format,
    register_format_with_replication, DeviceFormatError, DevicePayloadIngestError,
};
use fe_database::handlers::iot_reading::insert_readings_with_replication;
use fe_database::handlers::replicated_row::apply_replicated_row_handler;
use fe_database::{BlobStore, BlobStoreHandle};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{
    DeviceFormatRejection, DevicePayloadIngestRejection, DevicePayloadInput, IotReadingInput,
    ReplicatedRowOutcome, SourceClass, DEVICE_PAYLOAD_MAX_ASSETS,
};
use fe_sdk::device_format::DeviceFormatDescriptor;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

const VERSE: &str = "verse-1";
const FRACTAL: &str = "fractal-1";
const PETAL: &str = "petal-1";
const OTHER_VERSE: &str = "verse-2";
const OTHER_PETAL: &str = "petal-2";
const FORMAT_MIME: &str = "application/octet-stream";
/// A client-chosen retry id (the door accepts only ULIDs).
const CLIENT_PAYLOAD_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

async fn setup_db() -> Db {
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

/// Seed a full verse → fractal → petal → node chain.
async fn seed_hierarchy(db: &Db, verse_id: &str, fractal_id: &str, petal_id: &str, node_id: &str) {
    db.query(
        "CREATE verse CONTENT { verse_id: $vid, name: 'v', created_by: 'did:key:z6MkOwner', \
         created_at: '2026-10-14T00:00:00Z', default_access: 'viewer' };
         CREATE fractal CONTENT { fractal_id: $fid, verse_id: $vid, owner_did: 'did:key:z6MkOwner', \
         name: 'f', created_at: '2026-10-14T00:00:00Z' };
         CREATE petal CONTENT { petal_id: $pid, fractal_id: $fid, name: 'p', \
         created_at: '2026-10-14T00:00:00Z' };
         CREATE node CONTENT { node_id: $nid, petal_id: $pid, display_name: 'anchor', \
         position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
         scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-14T00:00:00Z' };",
    )
    .bind(("vid", verse_id.to_string()))
    .bind(("fid", fractal_id.to_string()))
    .bind(("pid", petal_id.to_string()))
    .bind(("nid", node_id.to_string()))
    .await
    .expect("seed hierarchy")
    .check()
    .expect("seed hierarchy check");
}

async fn seed_role(db: &Db, peer_did: &str, verse_id: &str, role: &str) {
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
        .bind(("did", peer_did.to_string()))
        .bind(("scope", format!("VERSE#{verse_id}")))
        .bind(("role", role.to_string()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");
}

/// Store `bytes` and register a matching asset row anchored to `petal_id`.
async fn seed_asset(
    db: &Db,
    store: &BlobStoreHandle,
    bytes: &[u8],
    petal_id: &str,
    content_type: &str,
) -> String {
    let hash = fe_database::hash_to_hex(&store.add_blob(bytes).unwrap());
    let asset_id = ulid::Ulid::new().to_string();
    db.query(
        "CREATE asset CONTENT { asset_id: $aid, name: 'f.bin', content_type: $ct, \
         size_bytes: $size, content_hash: $hash, owner_did: 'did:key:z6MkEditor', \
         petal_id: $pid, created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("aid", asset_id.clone()))
    .bind(("ct", content_type.to_string()))
    .bind(("size", bytes.len() as i64))
    .bind(("hash", hash))
    .bind(("pid", petal_id.to_string()))
    .await
    .unwrap()
    .check()
    .unwrap();
    asset_id
}

async fn declare_format(db: &Db, verse_id: &str, format_id: &str, mime: &str) {
    register_format(
        db,
        verse_id,
        &DeviceFormatDescriptor {
            format_id: format_id.to_string(),
            mime: mime.to_string(),
            schema: None,
            description: String::new(),
        },
        "did:key:z6MkManager",
    )
    .await
    .expect("declare format");
}

fn blob_store() -> (Arc<MockBlobStore>, BlobStoreHandle) {
    let store = Arc::new(MockBlobStore::new());
    let handle: BlobStoreHandle = store.clone();
    (store, handle)
}

/// SPEC-10 §8 rule 2 (CDL-SPEC10 B3): the door records window membership only in the shadow
/// modes. Every test in this binary that sets the process-global mode sets this same value and
/// none clears it, so concurrent tests never observe a different answer.
fn enable_shadow_window_membership() {
    std::env::set_var(
        fe_database::migration::mode::MIGRATION_MODE_ENV_VAR,
        "dual_emit_shadow",
    );
}

async fn window_members(db: &Db, table: &str, row_id: &str) -> Vec<serde_json::Value> {
    let mut res = db
        .query("SELECT * FROM observation_window_member WHERE table = $t AND row_id = $r")
        .bind(("t", table.to_string()))
        .bind(("r", row_id.to_string()))
        .await
        .unwrap();
    res.take(0).unwrap()
}

// ---------------------------------------------------------------------------
// Ingest + list round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn device_payload_ingest_records_asset_list_and_lists_it_back() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let a1 = seed_asset(&db, &handle, b"blob one", PETAL, FORMAT_MIME).await;
    let a2 = seed_asset(&db, &handle, b"blob two", PETAL, FORMAT_MIME).await;

    let (payload_id, asset_ids) = insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![a1.clone(), a2.clone()],
            recorded_at: None,
            payload_id: None,
        },
        Some(&handle),
        None,
    )
    .await
    .expect("ingest payload");

    assert_eq!(asset_ids, vec![a1.clone(), a2.clone()], "order preserved");

    let listed = list_device_payloads(&db, PETAL, Some("node-a"), None, None, None)
        .await
        .expect("list by node");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].payload_id, payload_id);
    assert_eq!(listed[0].asset_ids, vec![a1, a2], "list keeps ingest order");
}

// ---------------------------------------------------------------------------
// Validation rejections (all-or-nothing)
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::type_complexity)]
async fn invalid_payload_batch_writes_nothing() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_hierarchy(&db, OTHER_VERSE, "fractal-2", OTHER_PETAL, "node-b").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let good_asset = seed_asset(&db, &handle, b"good", PETAL, FORMAT_MIME).await;
    let foreign_asset = seed_asset(&db, &handle, b"foreign", OTHER_PETAL, FORMAT_MIME).await;
    let wrong_mime_asset = seed_asset(&db, &handle, b"wrong mime", PETAL, "image/png").await;

    let cases: Vec<(
        &str,
        DevicePayloadInput,
        fn(&DevicePayloadIngestError) -> bool,
    )> = vec![
        (
            "unknown format",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.unknown/frame@1".to_string(),
                asset_ids: vec![good_asset.clone()],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::UnknownFormat { .. })
                )
            },
        ),
        (
            "anchor in another petal",
            DevicePayloadInput {
                node_id: "node-b".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![good_asset.clone()],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::UnknownAnchor { .. })
                )
            },
        ),
        (
            "asset anchored to a foreign petal",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![foreign_asset.clone()],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::ForeignAssetAnchor { .. })
                )
            },
        ),
        (
            "MIME/descriptor mismatch",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![wrong_mime_asset.clone()],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::MimeMismatch { .. })
                )
            },
        ),
        (
            "empty asset list",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::EmptyAssetList)
                )
            },
        ),
        (
            "bad timestamp",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![good_asset.clone()],
                recorded_at: Some("not-a-timestamp".to_string()),
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::InvalidTimestamp { .. })
                )
            },
        ),
        (
            "client payload_id is not a ULID",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![good_asset.clone()],
                recorded_at: None,
                payload_id: Some("client-chosen-1".to_string()),
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::InvalidPayloadId { .. })
                )
            },
        ),
        (
            "more asset ids than the shared cap",
            DevicePayloadInput {
                node_id: "node-a".to_string(),
                format_id: "acme.thermo/frame@1".to_string(),
                asset_ids: vec![good_asset.clone(); DEVICE_PAYLOAD_MAX_ASSETS + 1],
                recorded_at: None,
                payload_id: None,
            },
            |e| {
                matches!(
                    e.rejection(),
                    Some(DevicePayloadIngestRejection::TooManyAssets { .. })
                )
            },
        ),
    ];

    for (name, input, matches_rejection) in cases {
        let err = insert_device_payloads(
            &db,
            PETAL,
            Some(VERSE),
            "did:key:z6MkEditor",
            SourceClass::Device,
            &input,
            Some(&handle),
            None,
        )
        .await
        .expect_err(name);
        assert!(matches_rejection(&err), "{name}: wrong rejection {err:?}");
    }

    let rows = list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "every invalid case left zero device_payload rows"
    );
}

#[tokio::test]
async fn retried_payload_with_same_client_id_is_idempotent() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let asset_id = seed_asset(&db, &handle, b"bytes", PETAL, FORMAT_MIME).await;
    let input = DevicePayloadInput {
        node_id: "node-a".to_string(),
        format_id: "acme.thermo/frame@1".to_string(),
        asset_ids: vec![asset_id],
        recorded_at: None,
        payload_id: Some(CLIENT_PAYLOAD_ID.to_string()),
    };

    let (first_id, _) = insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &input,
        Some(&handle),
        None,
    )
    .await
    .expect("first ingest");
    let (second_id, _) = insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &input,
        Some(&handle),
        None,
    )
    .await
    .expect("retried ingest");

    assert_eq!(first_id, second_id);
    assert_eq!(first_id, CLIENT_PAYLOAD_ID);
    let rows = list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "idempotent retry never duplicates the row");
}

// ---------------------------------------------------------------------------
// GC interaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn payload_referenced_asset_survives_gc_and_orphan_is_collected() {
    let db = setup_db().await;
    let (store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let referenced = seed_asset(&db, &handle, b"referenced", PETAL, FORMAT_MIME).await;
    let orphan = seed_asset(&db, &handle, b"orphan", PETAL, FORMAT_MIME).await;
    // Backdate both rows' `created_at` so grace=0 makes them GC-eligible by age.
    db.query("UPDATE asset SET created_at = '2020-01-01T00:00:00Z'")
        .await
        .unwrap()
        .check()
        .unwrap();
    insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![referenced.clone()],
            recorded_at: None,
            payload_id: None,
        },
        Some(&handle),
        None,
    )
    .await
    .expect("ingest referencing one asset");

    let report = fe_database::handlers::asset_gc::gc_unreferenced_assets(&db, &handle, 0)
        .await
        .expect("gc pass");

    assert_eq!(report.removed_assets, 1, "only the orphan row is collected");
    let mut res = db.query("SELECT VALUE asset_id FROM asset").await.unwrap();
    let remaining: Vec<String> = res.take(0).unwrap();
    assert!(
        remaining.contains(&referenced),
        "payload-referenced asset survives"
    );
    assert!(!remaining.contains(&orphan), "the true orphan is gone");
    assert!(
        !store.has_blob(blake3::hash(b"orphan").as_bytes()),
        "the orphan's blob is gone too"
    );
    assert!(
        store.has_blob(blake3::hash(b"referenced").as_bytes()),
        "the referenced blob survives"
    );
}

// ---------------------------------------------------------------------------
// iot_reading <-> device_payload link
// ---------------------------------------------------------------------------

#[tokio::test]
async fn readings_link_to_their_payload() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let asset_id = seed_asset(&db, &handle, b"bytes", PETAL, FORMAT_MIME).await;
    let (payload_id, _) = insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![asset_id],
            recorded_at: None,
            payload_id: None,
        },
        Some(&handle),
        None,
    )
    .await
    .expect("ingest payload");

    let written = insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 21.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: Some(payload_id.clone()),
        }],
        Some(&handle),
        None,
    )
    .await
    .expect("linked reading accepted");
    assert_eq!(written, 1);

    let err = insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 22.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: Some("no-such-payload".to_string()),
        }],
        Some(&handle),
        None,
    )
    .await
    .expect_err("unknown payload_id must reject the batch");
    assert!(matches!(
        err,
        fe_database::handlers::iot_reading::IotIngestError::UnknownPayload(_)
    ));
}

// ---------------------------------------------------------------------------
// Inbound replication (A3 admission gate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inbound_anchored_asset_row_insert_if_absent() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    seed_role(&db, "did:key:z6MkViewer", VERSE, "viewer").await;
    let row = serde_json::json!({
        "asset_id": "asset-remote-1",
        "name": "remote.bin",
        "content_type": FORMAT_MIME,
        "size_bytes": 3,
        "content_hash": "ab".repeat(32),
        "owner_did": "did:key:z6MkEditor",
        "petal_id": PETAL,
        "created_at": "2026-10-14T00:00:00Z",
    });
    let bytes = serde_json::to_vec(&row).unwrap();

    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-remote-1",
        &bytes,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Applied);

    // Re-delivery with a DIFFERENT recorded hash never overwrites.
    let mut newer = row.clone();
    newer["content_hash"] = serde_json::json!("cd".repeat(32));
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-remote-1",
        &serde_json::to_vec(&newer).unwrap(),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Applied,
        "idempotent re-delivery"
    );
    let mut res = db
        .query("SELECT content_hash FROM asset WHERE asset_id = 'asset-remote-1'")
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap();
    assert_eq!(
        rows[0]["content_hash"],
        "ab".repeat(32),
        "never overwritten"
    );

    // An empty `del` is NotApplicable (no tombstone semantics on asset).
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-remote-1",
        &[],
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::NotApplicable);

    // A Viewer-authored row is denied.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-remote-2",
        &serde_json::to_vec(&serde_json::json!({
            "asset_id": "asset-remote-2", "name": "x", "content_type": FORMAT_MIME,
            "size_bytes": 1, "content_hash": "ef".repeat(32), "petal_id": PETAL,
            "created_at": "2026-10-14T00:00:00Z",
        }))
        .unwrap(),
        "did:key:z6MkViewer",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Denied);
}

#[tokio::test]
async fn inbound_device_payload_is_union_append_only() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    seed_role(&db, "did:key:z6MkViewer", VERSE, "viewer").await;
    // The format arrives first (its own door replicates it), then the
    // referenced asset (production order: asset rows apply before the payload
    // row — see the emission-order test).
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    db.query(
        "CREATE asset CONTENT { asset_id: 'asset-local-1', name: 'a', \
         content_type: $ct, size_bytes: 1, content_hash: $h, petal_id: $pid, \
         created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("ct", FORMAT_MIME.to_string()))
    .bind(("h", "11".repeat(32)))
    .bind(("pid", PETAL.to_string()))
    .await
    .unwrap()
    .check()
    .unwrap();

    let row = serde_json::json!({
        "payload_id": "payload-remote-1",
        "node_id": "node-a",
        "petal_id": PETAL,
        "verse_id": VERSE,
        "format_id": "acme.thermo/frame@1",
        "recorded_at": "2026-10-14T00:00:00Z",
        "recorded_at_ms": 0,
        "hlc_timestamp": 0,
        "asset_ids": ["asset-local-1"],
        "source_did": "did:key:z6MkEditor",
    });
    let bytes = serde_json::to_vec(&row).unwrap();

    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-remote-1",
        &bytes,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Applied);

    // Re-delivery is idempotent.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-remote-1",
        &bytes,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Applied);
    let listed = list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "union re-delivery never duplicates");

    // An empty `del` keeps the fact (union semantics).
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-remote-1",
        &[],
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::NotApplicable);

    // A Viewer-authored row is denied.
    let mut viewer_row = row.clone();
    viewer_row["payload_id"] = serde_json::json!("payload-remote-2");
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-remote-2",
        &serde_json::to_vec(&viewer_row).unwrap(),
        "did:key:z6MkViewer",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Denied);
}

/// Real `repl_tx` capture: asset rows emit before the payload row, after the
/// durable insert (SPEC-10 §2.3 ingestion-story order).
#[tokio::test]
async fn payload_emits_asset_rows_before_payload_row_after_durable_insert() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let asset_id = seed_asset(&db, &handle, b"bytes", PETAL, FORMAT_MIME).await;
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);

    insert_device_payloads(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![asset_id],
            recorded_at: None,
            payload_id: None,
        },
        Some(&handle),
        Some(&repl_tx),
    )
    .await
    .expect("ingest with replication");

    let mut events = Vec::new();
    while let Ok(evt) = repl_rx.try_recv() {
        events.push(evt);
    }
    assert_eq!(
        events.len(),
        2,
        "one event for the asset, one for the payload"
    );
    assert_eq!(events[0].table, "asset");
    assert_eq!(events[1].table, "device_payload");
}

/// Critic C4: every inbound arm confines its references to the replica's
/// verse — a reference to another verse is denied.
#[tokio::test]
async fn inbound_device_rows_referencing_another_verse_are_denied() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_hierarchy(&db, OTHER_VERSE, "fractal-2", OTHER_PETAL, "node-b").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    seed_role(&db, "did:key:z6MkManager", VERSE, "manager").await;
    db.query(
        "CREATE asset CONTENT { asset_id: 'asset-foreign', name: 'a', content_type: $ct, \
         size_bytes: 1, content_hash: $h, petal_id: $pid, created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("ct", FORMAT_MIME.to_string()))
    .bind(("h", "22".repeat(32)))
    .bind(("pid", OTHER_PETAL.to_string()))
    .await
    .unwrap()
    .check()
    .unwrap();

    // asset.petal_id in another verse.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-x",
        &serde_json::to_vec(&serde_json::json!({
            "asset_id": "asset-x", "name": "a", "content_type": FORMAT_MIME, "size_bytes": 1,
            "content_hash": "33".repeat(32), "petal_id": OTHER_PETAL,
            "created_at": "2026-10-14T00:00:00Z",
        }))
        .unwrap(),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "asset.petal_id in another verse"
    );

    // device_payload.petal_id in another verse.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-x",
        &serde_json::to_vec(&serde_json::json!({
            "payload_id": "payload-x", "node_id": "node-b", "petal_id": OTHER_PETAL,
            "verse_id": VERSE, "format_id": "acme.thermo/frame@1",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0, "hlc_timestamp": 0,
            "asset_ids": [], "source_did": "did:key:z6MkEditor",
        }))
        .unwrap(),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "device_payload.petal_id in another verse"
    );

    // device_payload.asset_ids entry anchored elsewhere.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-y",
        &serde_json::to_vec(&serde_json::json!({
            "payload_id": "payload-y", "node_id": "node-a", "petal_id": PETAL,
            "verse_id": VERSE, "format_id": "acme.thermo/frame@1",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0, "hlc_timestamp": 0,
            "asset_ids": ["asset-foreign"], "source_did": "did:key:z6MkEditor",
        }))
        .unwrap(),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "device_payload.asset_ids entry anchored elsewhere"
    );

    // device_format.verse_id mismatch.
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_format",
        "acme.thermo/frame@1",
        &serde_json::to_vec(&serde_json::json!({
            "format_id": "acme.thermo/frame@1", "verse_id": OTHER_VERSE, "mime": FORMAT_MIME,
            "schema": null, "declared_by": "did:key:z6MkManager", "created_at": "2026-10-14T00:00:00Z",
        })).unwrap(),
        "did:key:z6MkManager",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "device_format.verse_id mismatch"
    );
}

/// Critic C3 ("strictest reader"): `device_format` needs Manager+, not just
/// Editor+ like `asset`/`device_payload`.
#[tokio::test]
async fn an_editor_authored_inbound_device_format_is_denied_and_a_manager_one_applies() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    seed_role(&db, "did:key:z6MkManager", VERSE, "manager").await;
    let row = |format_id: &str| {
        serde_json::to_vec(&serde_json::json!({
            "format_id": format_id, "verse_id": VERSE, "mime": FORMAT_MIME,
            "schema": null, "declared_by": "did:key:z6MkManager",
            "created_at": "2026-10-14T00:00:00Z",
        }))
        .unwrap()
    };

    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_format",
        "acme.thermo/editor@1",
        &row("acme.thermo/editor@1"),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "Editor is below the device_format floor"
    );

    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_format",
        "acme.thermo/manager@1",
        &row("acme.thermo/manager@1"),
        "did:key:z6MkManager",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Applied,
        "Manager clears the device_format floor"
    );
}

/// Critic tick 7 AC-34: role-independent checks run before the bootstrap
/// window — without ANY local knowledge of the verse/petal/node/assets, all
/// three new tables fail closed, not open.
#[tokio::test]
async fn pre_manifest_device_rows_follow_the_declared_bootstrap_rule() {
    let db = setup_db().await; // schema applied, but NO verse/petal/node rows at all.

    let asset_row = serde_json::to_vec(&serde_json::json!({
        "asset_id": "asset-pm", "name": "a", "content_type": FORMAT_MIME, "size_bytes": 1,
        "content_hash": "44".repeat(32), "petal_id": "petal-unknown",
        "created_at": "2026-10-14T00:00:00Z",
    }))
    .unwrap();
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-pm",
        &asset_row,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "asset fails closed pre-manifest"
    );

    let format_row = serde_json::to_vec(&serde_json::json!({
        "format_id": "acme.thermo/pm@1", "verse_id": VERSE, "mime": FORMAT_MIME,
        "schema": null, "declared_by": "did:key:z6MkManager", "created_at": "2026-10-14T00:00:00Z",
    }))
    .unwrap();
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_format",
        "acme.thermo/pm@1",
        &format_row,
        "did:key:z6MkManager",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "device_format fails closed pre-manifest"
    );

    let payload_row = serde_json::to_vec(&serde_json::json!({
        "payload_id": "payload-pm", "node_id": "node-unknown", "petal_id": "petal-unknown",
        "verse_id": VERSE, "format_id": "acme.thermo/pm@1", "recorded_at": "2026-10-14T00:00:00Z",
        "recorded_at_ms": 0, "hlc_timestamp": 0, "asset_ids": [], "source_did": "did:key:z6MkEditor",
    }))
    .unwrap();
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "device_payload",
        "payload-pm",
        &payload_row,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Denied,
        "device_payload is denied when its petal/node/assets are not locally known"
    );
}

// ---------------------------------------------------------------------------
// SPEC-10 §3.4 rule 5: window membership is door-only, never inbound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inbound_replicated_rows_never_enter_window_membership() {
    enable_shadow_window_membership();
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkLocalNode", VERSE, "editor").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;

    // A malicious peer replicates an iot_reading row carrying THIS NODE'S
    // OWN DID as source_did — the signing-oracle gap this table must resist.
    let reading_row = serde_json::json!({
        "reading_id": "reading-inbound-1", "node_id": "node-a", "petal_id": PETAL,
        "metric": "temperature_c", "value": 1.0, "units": "C",
        "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0, "hlc_timestamp": 0,
        "source_did": "did:key:z6MkLocalNode", "payload_id": null,
    });
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "iot_reading",
        "reading-inbound-1",
        &serde_json::to_vec(&reading_row).unwrap(),
        "did:key:z6MkLocalNode",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        ReplicatedRowOutcome::Applied,
        "the row itself still applies (union)"
    );
    assert!(
        window_members(&db, "iot_reading", "reading-inbound-1")
            .await
            .is_empty(),
        "the inbound path must NEVER write window membership"
    );

    // The locally-door-accepted sibling, same window, DOES appear.
    insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkLocalNode",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 2.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: None,
        }],
        Some(&handle),
        None,
    )
    .await
    .expect("local door ingest");
    let mut res = db
        .query("SELECT VALUE reading_id FROM iot_reading WHERE value = 2.0")
        .await
        .unwrap();
    let ids: Vec<String> = res.take(0).unwrap();
    let local_reading_id = ids.into_iter().next().expect("local reading written");
    assert!(
        !window_members(&db, "iot_reading", &local_reading_id)
            .await
            .is_empty(),
        "the locally-accepted sibling DOES appear in window membership"
    );
}

// ---------------------------------------------------------------------------
// A1 hand-back item 4: explicit source_class, never inferred from source_did
// ---------------------------------------------------------------------------

#[tokio::test]
async fn simulated_readings_carry_source_class_simulation_not_inferred_from_did() {
    enable_shadow_window_membership();
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;

    // Built the way `fe-sim/src/session.rs` builds it: `source_did` is the
    // simulation lab's HOST PEER DID, which SPEC-10 §7.1 rule 6 forbids
    // reading as `device`/`connector`.
    insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkHostPeer",
        SourceClass::Simulation,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 1.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: None,
        }],
        Some(&handle),
        None,
    )
    .await
    .expect("simulated reading");
    insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkRestCaller",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 2.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: None,
        }],
        Some(&handle),
        None,
    )
    .await
    .expect("ordinary reading");

    let mut res = db
        .query("SELECT reading_id, value, source_did FROM iot_reading ORDER BY value ASC")
        .await
        .unwrap();
    let readings: Vec<serde_json::Value> = res.take(0).unwrap();
    assert_eq!(readings.len(), 2);

    let sim_reading_id = readings[0]["reading_id"].as_str().unwrap();
    let rest_reading_id = readings[1]["reading_id"].as_str().unwrap();
    let sim_members = window_members(&db, "iot_reading", sim_reading_id).await;
    let rest_members = window_members(&db, "iot_reading", rest_reading_id).await;
    assert_eq!(sim_members[0]["source_class"], "simulation");
    assert_eq!(rest_members[0]["source_class"], "device");
}

// ---------------------------------------------------------------------------
// DeviceFormat registration (idempotence + conflict)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_format_conflict_and_idempotent_redeclare() {
    let db = setup_db().await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;

    // Re-declaring the SAME mime is an idempotent no-op.
    register_format(
        &db,
        VERSE,
        &DeviceFormatDescriptor {
            format_id: "acme.thermo/frame@1".to_string(),
            mime: FORMAT_MIME.to_string(),
            schema: None,
            description: String::new(),
        },
        "did:key:z6MkManager",
    )
    .await
    .expect("idempotent re-declare");

    // A DIFFERENT mime is a conflict.
    let err = register_format(
        &db,
        VERSE,
        &DeviceFormatDescriptor {
            format_id: "acme.thermo/frame@1".to_string(),
            mime: "image/png".to_string(),
            schema: None,
            description: String::new(),
        },
        "did:key:z6MkManager",
    )
    .await
    .expect_err("conflicting mime must be refused");
    assert!(matches!(
        err,
        DeviceFormatError::Rejected(DeviceFormatRejection::ConflictingMime { .. })
    ));
}

/// RP-0 (critic tick 8, AC-49): a descriptor `schema` that is present but not a JSON
/// object (array, string, number, bool) is a typed `InvalidSchema` rejection, not an
/// untyped fall-through to a DB-layer failure — row added to this door's rejection
/// cases alongside `register_format_conflict_and_idempotent_redeclare` above.
#[tokio::test]
async fn register_format_rejects_a_non_object_schema() {
    let db = setup_db().await;
    let cases = [
        ("array", serde_json::json!(["type", "object"])),
        ("string", serde_json::json!("object")),
        ("number", serde_json::json!(1)),
        ("bool", serde_json::json!(true)),
    ];
    for (name, schema) in cases {
        let err = register_format(
            &db,
            VERSE,
            &DeviceFormatDescriptor {
                format_id: "acme.thermo/shape@1".to_string(),
                mime: FORMAT_MIME.to_string(),
                schema: Some(schema),
                description: String::new(),
            },
            "did:key:z6MkManager",
        )
        .await
        .expect_err(&format!("{name}: a non-object schema must be refused"));
        assert!(
            matches!(
                err,
                DeviceFormatError::Rejected(DeviceFormatRejection::InvalidSchema)
            ),
            "{name}: {err:?}"
        );
    }
    assert!(
        list_device_formats(&db, VERSE).await.unwrap().is_empty(),
        "nothing was written for any rejected descriptor"
    );

    // A `schema` that IS an object is still accepted (unchanged behaviour).
    register_format(
        &db,
        VERSE,
        &DeviceFormatDescriptor {
            format_id: "acme.thermo/shape@1".to_string(),
            mime: FORMAT_MIME.to_string(),
            schema: Some(serde_json::json!({"type": "object"})),
            description: String::new(),
        },
        "did:key:z6MkManager",
    )
    .await
    .expect("an object schema is accepted");
}

// ---------------------------------------------------------------------------
// Phase A security review-and-fix (CDL-SPEC10, 2026-10-15)
// ---------------------------------------------------------------------------

/// Ingest one single-asset payload into `petal_id` as `source_did`.
#[allow(clippy::too_many_arguments)]
async fn ingest_one(
    db: &Db,
    handle: &BlobStoreHandle,
    petal_id: &str,
    verse_id: &str,
    node_id: &str,
    asset_id: &str,
    source_did: &str,
    payload_id: Option<&str>,
) -> Result<(String, Vec<String>), DevicePayloadIngestError> {
    insert_device_payloads(
        db,
        petal_id,
        Some(verse_id),
        source_did,
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: node_id.to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![asset_id.to_string()],
            recorded_at: None,
            payload_id: payload_id.map(str::to_string),
        },
        Some(handle),
        None,
    )
    .await
}

/// MANDATORY finding 2 (IDOR on idempotent retry): a known client
/// `payload_id` is a retry only for the same petal AND the same principal.
/// Anyone else replaying the id gets a conflict that carries no asset ids, and
/// nothing is written. Mutation check: restoring the unscoped
/// return-existing-row branch hands the foreign caller petal Q's asset ids.
#[tokio::test]
async fn retry_from_another_petal_or_principal_gets_a_conflict_and_no_asset_ids() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_hierarchy(&db, OTHER_VERSE, "fractal-2", OTHER_PETAL, "node-b").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    declare_format(&db, OTHER_VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let victim_asset = seed_asset(&db, &handle, b"victim", PETAL, FORMAT_MIME).await;
    let attacker_asset = seed_asset(&db, &handle, b"attacker", OTHER_PETAL, FORMAT_MIME).await;

    let (victim_id, victim_assets) = ingest_one(
        &db,
        &handle,
        PETAL,
        VERSE,
        "node-a",
        &victim_asset,
        "did:key:z6MkVictim",
        Some(CLIENT_PAYLOAD_ID),
    )
    .await
    .expect("victim ingest");
    assert_eq!(victim_assets, vec![victim_asset.clone()]);

    // Another petal replays the id: conflict, no asset ids, no second row.
    let err = ingest_one(
        &db,
        &handle,
        OTHER_PETAL,
        OTHER_VERSE,
        "node-b",
        &attacker_asset,
        "did:key:z6MkAttacker",
        Some(&victim_id),
    )
    .await
    .expect_err("a foreign petal never gets the existing row");
    assert_eq!(
        err.rejection(),
        Some(DevicePayloadIngestRejection::PayloadIdConflict)
    );
    assert!(
        !format!("{err:?}").contains(&victim_asset),
        "the conflict names nothing about the existing row: {err:?}"
    );

    // Same petal, different principal: also a conflict.
    let err = ingest_one(
        &db,
        &handle,
        PETAL,
        VERSE,
        "node-a",
        &victim_asset,
        "did:key:z6MkOtherEditor",
        Some(&victim_id),
    )
    .await
    .expect_err("another principal's retry is not a retry");
    assert_eq!(
        err.rejection(),
        Some(DevicePayloadIngestRejection::PayloadIdConflict)
    );

    // The real retry still works.
    let (again, assets) = ingest_one(
        &db,
        &handle,
        PETAL,
        VERSE,
        "node-a",
        &victim_asset,
        "did:key:z6MkVictim",
        Some(&victim_id),
    )
    .await
    .expect("same petal + same principal is an idempotent retry");
    assert_eq!(again, victim_id);
    assert_eq!(assets, vec![victim_asset]);

    assert!(
        list_device_payloads(&db, OTHER_PETAL, None, None, None, None)
            .await
            .unwrap()
            .is_empty(),
        "nothing was written into the replaying petal"
    );
}

/// The door publishes a payload only into its petal's OWN verse: a caller
/// naming a foreign verse as the replication target is refused, nothing is
/// written and nothing is emitted.
#[tokio::test]
async fn payload_naming_a_foreign_replication_verse_is_refused() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_hierarchy(&db, OTHER_VERSE, "fractal-2", OTHER_PETAL, "node-b").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let asset_id = seed_asset(&db, &handle, b"bytes", PETAL, FORMAT_MIME).await;
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);

    let err = insert_device_payloads(
        &db,
        PETAL,
        Some(OTHER_VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![asset_id],
            recorded_at: None,
            payload_id: None,
        },
        Some(&handle),
        Some(&repl_tx),
    )
    .await
    .expect_err("a foreign replication verse is refused");
    assert!(matches!(err, DevicePayloadIngestError::Db(_)), "{err:?}");
    assert!(repl_rx.try_recv().is_err(), "nothing was emitted");
    assert!(list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap()
        .is_empty());
}

/// MANDATORY finding 1 (cross-tenant reference): a reading ingested into
/// petal P that names petal Q's payload is refused as unknown and nothing is
/// written, so no reading ever links across petals. Mutation check: dropping
/// the petal filter from `fetch_known_payloads` admits the batch.
#[tokio::test]
async fn reading_cannot_link_to_another_petals_payload() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_hierarchy(&db, OTHER_VERSE, "fractal-2", OTHER_PETAL, "node-b").await;
    declare_format(&db, OTHER_VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let foreign_asset = seed_asset(&db, &handle, b"foreign", OTHER_PETAL, FORMAT_MIME).await;
    let (foreign_payload, _) = ingest_one(
        &db,
        &handle,
        OTHER_PETAL,
        OTHER_VERSE,
        "node-b",
        &foreign_asset,
        "did:key:z6MkOther",
        None,
    )
    .await
    .expect("foreign payload");

    let err = insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkEditor",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 21.0,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: Some(foreign_payload),
        }],
        Some(&handle),
        None,
    )
    .await
    .expect_err("a foreign petal's payload is unknown here");
    assert!(matches!(
        err,
        fe_database::handlers::iot_reading::IotIngestError::UnknownPayload(_)
    ));
    let mut res = db.query("SELECT * FROM iot_reading").await.unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    assert!(rows.is_empty(), "nothing links across petals");
}

/// The inbound twin of the finding above: a replicated reading whose
/// `payload_id` names another petal's payload is denied, while one linking
/// its own petal's payload applies.
#[tokio::test]
async fn inbound_reading_linking_a_foreign_petal_payload_is_denied() {
    let db = setup_db().await;
    let (_store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    db.query(
        "CREATE petal CONTENT { petal_id: 'petal-3', fractal_id: $fid, name: 'p3', \
         created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("fid", FRACTAL.to_string()))
    .await
    .unwrap()
    .check()
    .unwrap();
    db.query(
        "CREATE node CONTENT { node_id: 'node-c', petal_id: 'petal-3', display_name: 'c', \
         position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
         scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-14T00:00:00Z' }",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let asset_c = seed_asset(&db, &handle, b"c", "petal-3", FORMAT_MIME).await;
    let (payload_c, _) = ingest_one(
        &db,
        &handle,
        "petal-3",
        VERSE,
        "node-c",
        &asset_c,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .expect("petal-3 payload");
    let asset_a = seed_asset(&db, &handle, b"a", PETAL, FORMAT_MIME).await;
    let (payload_a, _) = ingest_one(
        &db,
        &handle,
        PETAL,
        VERSE,
        "node-a",
        &asset_a,
        "did:key:z6MkEditor",
        None,
    )
    .await
    .expect("petal-1 payload");

    let reading = |reading_id: &str, payload_id: &str| {
        serde_json::to_vec(&serde_json::json!({
            "reading_id": reading_id, "node_id": "node-a", "petal_id": PETAL,
            "metric": "t", "value": 1.0, "units": "C",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0,
            "hlc_timestamp": 0, "source_did": "did:key:z6MkEditor",
            "payload_id": payload_id,
        }))
        .unwrap()
    };

    let foreign = apply_replicated_row_handler(
        &db,
        VERSE,
        "iot_reading",
        "reading-foreign",
        &reading("reading-foreign", &payload_c),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(foreign, ReplicatedRowOutcome::Denied);

    let own = apply_replicated_row_handler(
        &db,
        VERSE,
        "iot_reading",
        "reading-own",
        &reading("reading-own", &payload_a),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(own, ReplicatedRowOutcome::Applied);
}

/// Inbound rows whose references or own fields cannot be read are denied
/// instead of skipped (table-driven over each shape hole the review found).
/// Mutation check: restoring any `continue`/`if let Some` skip turns its row
/// from `Denied` to `Applied`.
#[tokio::test]
async fn inbound_device_rows_with_unreadable_fields_are_denied() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkManager", VERSE, "manager").await;
    // Declared so each payload case below is denied by ITS named defect, not
    // by the format/MIME confinement check that runs last.
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    db.query(
        "CREATE asset CONTENT { asset_id: 'asset-local-1', name: 'a', \
         content_type: $ct, size_bytes: 1, content_hash: $h, petal_id: $pid, \
         created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("ct", FORMAT_MIME.to_string()))
    .bind(("h", "11".repeat(32)))
    .bind(("pid", PETAL.to_string()))
    .await
    .unwrap()
    .check()
    .unwrap();

    let payload = |patch: serde_json::Value| {
        let mut row = serde_json::json!({
            "payload_id": "payload-shape", "node_id": "node-a", "petal_id": PETAL,
            "verse_id": VERSE, "format_id": "acme.thermo/frame@1",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0,
            "hlc_timestamp": 0, "asset_ids": ["asset-local-1"], "source_did": "x",
        });
        for (key, value) in patch.as_object().unwrap() {
            if value.is_null() {
                row.as_object_mut().unwrap().remove(key);
            } else {
                row[key] = value.clone();
            }
        }
        row
    };
    let asset = |patch: serde_json::Value| {
        let mut row = serde_json::json!({
            "asset_id": "asset-shape", "name": "x", "content_type": FORMAT_MIME,
            "size_bytes": 3, "content_hash": "ab".repeat(32), "petal_id": PETAL,
            "created_at": "2026-10-14T00:00:00Z",
        });
        for (key, value) in patch.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };
    let format = |patch: serde_json::Value| {
        let mut row = serde_json::json!({
            "format_id": "acme.thermo/shape@1", "verse_id": VERSE, "mime": FORMAT_MIME,
            "declared_by": "did:key:z6MkManager", "created_at": "2026-10-14T00:00:00Z",
        });
        for (key, value) in patch.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "payload without node_id",
            "device_payload",
            payload(serde_json::json!({ "node_id": null })),
        ),
        (
            "payload without asset_ids",
            "device_payload",
            payload(serde_json::json!({ "asset_ids": null })),
        ),
        (
            "payload with empty asset_ids",
            "device_payload",
            payload(serde_json::json!({ "asset_ids": [] })),
        ),
        (
            "payload with a non-string asset id",
            "device_payload",
            payload(serde_json::json!({ "asset_ids": ["asset-local-1", 7] })),
        ),
        (
            "payload over the asset cap",
            "device_payload",
            payload(
                serde_json::json!({ "asset_ids": vec!["asset-local-1"; DEVICE_PAYLOAD_MAX_ASSETS + 1] }),
            ),
        ),
        (
            "payload claiming a foreign verse_id",
            "device_payload",
            payload(serde_json::json!({ "verse_id": OTHER_VERSE })),
        ),
        (
            "asset with a path-escaping id",
            "asset",
            asset(serde_json::json!({ "asset_id": "../../etc/passwd" })),
        ),
        (
            "asset with a malformed hash",
            "asset",
            asset(serde_json::json!({ "content_hash": "not-hex" })),
        ),
        (
            "asset with a negative size",
            "asset",
            asset(serde_json::json!({ "size_bytes": -1_000_000_000_i64 })),
        ),
        (
            "asset with a header-breaking content_type",
            "asset",
            asset(serde_json::json!({ "content_type": "text/html\r\nX: 1" })),
        ),
        (
            "format with a malformed format_id",
            "device_format",
            format(serde_json::json!({ "format_id": "not-a-format" })),
        ),
        (
            "format with a malformed mime",
            "device_format",
            format(serde_json::json!({ "mime": "nonsense" })),
        ),
    ];

    for (name, table, row) in cases {
        let record_id = row
            .get("payload_id")
            .or_else(|| row.get("asset_id"))
            .or_else(|| row.get("format_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("x")
            .to_string();
        let outcome = apply_replicated_row_handler(
            &db,
            VERSE,
            table,
            &record_id,
            &serde_json::to_vec(&row).unwrap(),
            "did:key:z6MkManager",
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(outcome, ReplicatedRowOutcome::Denied, "{name}");
    }

    for (table, key) in [
        ("device_payload", "payload_id"),
        ("asset", "asset_id"),
        ("device_format", "format_id"),
    ] {
        let mut res = db
            .query(format!("SELECT {key} FROM {table}"))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
        let planted = rows
            .iter()
            .filter(|r| {
                r[key]
                    .as_str()
                    .is_some_and(|id| id != "asset-local-1" && id != "acme.thermo/frame@1")
            })
            .count();
        assert_eq!(planted, 0, "no denied {table} row was stored: {rows:?}");
    }
}

/// An inbound asset row never charges (or credits) a local uploader's quota:
/// its `owner_did` is dropped on apply, so `SUM(size_bytes) WHERE owner_did`
/// cannot be moved by a peer.
#[tokio::test]
async fn inbound_asset_row_is_never_attributed_to_a_local_uploader() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    let row = serde_json::json!({
        "asset_id": "asset-quota-1", "name": "big.bin", "content_type": FORMAT_MIME,
        "size_bytes": 9_000_000_000_i64, "content_hash": "ab".repeat(32),
        "owner_did": "did:key:z6MkLocalVictim", "petal_id": PETAL,
        "created_at": "2026-10-14T00:00:00Z",
    });
    let outcome = apply_replicated_row_handler(
        &db,
        VERSE,
        "asset",
        "asset-quota-1",
        &serde_json::to_vec(&row).unwrap(),
        "did:key:z6MkEditor",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Applied);

    let mut res = db
        .query(
            "SELECT math::sum(size_bytes) AS used FROM asset \
             WHERE owner_did = 'did:key:z6MkLocalVictim' GROUP ALL",
        )
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    let used = rows.first().and_then(|r| r["used"].as_i64()).unwrap_or(0);
    assert_eq!(used, 0, "the peer's row counts against no local quota");
}

// ---------------------------------------------------------------------------
// CDL-SPEC10 review-and-fix (2026-10-15)
// ---------------------------------------------------------------------------

/// A format declared at the door reaches a peer: the door publishes the NEW row
/// (keyed by `format_id`), the peer's inbound gate applies those exact bytes, and
/// an idempotent re-declare publishes nothing.
#[tokio::test]
async fn declared_format_replicates_and_a_peer_applies_it() {
    let alice = setup_db().await;
    let bob = setup_db().await;
    let (store, handle) = blob_store();
    seed_hierarchy(&bob, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&bob, "did:key:z6MkManager", VERSE, "manager").await;
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);
    let descriptor = DeviceFormatDescriptor {
        format_id: "acme.thermo/frame@1".to_string(),
        mime: FORMAT_MIME.to_string(),
        schema: None,
        description: String::new(),
    };

    register_format_with_replication(
        &alice,
        VERSE,
        &descriptor,
        "did:key:z6MkManager",
        Some(&handle),
        Some(&repl_tx),
    )
    .await
    .expect("declare + publish");
    let event = repl_rx.try_recv().expect("the new format is published");
    assert!(repl_rx.try_recv().is_err(), "exactly one event");
    assert_eq!(event.verse_id, VERSE);
    assert_eq!(event.table, "device_format");
    assert_eq!(event.record_id, "acme.thermo/frame@1");

    let bytes = store
        .bytes_for(&event.content_hash)
        .expect("row bytes stored");
    let outcome = apply_replicated_row_handler(
        &bob,
        VERSE,
        "device_format",
        &event.record_id,
        &bytes,
        "did:key:z6MkManager",
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, ReplicatedRowOutcome::Applied);
    let formats = list_device_formats(&bob, VERSE).await.unwrap();
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0].format_id, "acme.thermo/frame@1");
    assert_eq!(formats[0].mime, FORMAT_MIME);

    register_format_with_replication(
        &alice,
        VERSE,
        &descriptor,
        "did:key:z6MkManager",
        Some(&handle),
        Some(&repl_tx),
    )
    .await
    .expect("idempotent re-declare");
    assert!(
        repl_rx.try_recv().is_err(),
        "a re-declare publishes nothing"
    );
}

/// SPEC-10 §2.2 rule 5 on the inbound path (strictest reader): a peer's payload
/// is denied while its format is undeclared locally and while a listed asset
/// carries another MIME; the same payload over a matching asset applies.
#[tokio::test]
async fn inbound_payload_needs_a_declared_format_and_its_mime() {
    let db = setup_db().await;
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    seed_role(&db, "did:key:z6MkEditor", VERSE, "editor").await;
    for (asset_id, content_type) in [("asset-html", "text/html"), ("asset-raw", FORMAT_MIME)] {
        db.query(
            "CREATE asset CONTENT { asset_id: $aid, name: 'a', content_type: $ct, \
             size_bytes: 1, content_hash: $h, petal_id: $pid, created_at: '2026-10-14T00:00:00Z' }",
        )
        .bind(("aid", asset_id.to_string()))
        .bind(("ct", content_type.to_string()))
        .bind(("h", "11".repeat(32)))
        .bind(("pid", PETAL.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    }
    let payload = |payload_id: &str, asset_id: &str| {
        serde_json::to_vec(&serde_json::json!({
            "payload_id": payload_id, "node_id": "node-a", "petal_id": PETAL,
            "verse_id": VERSE, "format_id": "acme.thermo/frame@1",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 0, "hlc_timestamp": 0,
            "asset_ids": [asset_id], "source_did": "did:key:z6MkEditor",
        }))
        .unwrap()
    };

    let cases = [
        (
            "payload-early",
            "asset-raw",
            false,
            ReplicatedRowOutcome::Denied,
            "undeclared format",
        ),
        (
            "payload-html",
            "asset-html",
            true,
            ReplicatedRowOutcome::Denied,
            "foreign MIME",
        ),
        (
            "payload-raw",
            "asset-raw",
            true,
            ReplicatedRowOutcome::Applied,
            "matching MIME",
        ),
    ];
    for (payload_id, asset_id, declared, expected, name) in cases {
        if declared {
            declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
        }
        let outcome = apply_replicated_row_handler(
            &db,
            VERSE,
            "device_payload",
            payload_id,
            &payload(payload_id, asset_id),
            "did:key:z6MkEditor",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, expected, "{name}");
    }
    let ids: Vec<String> = list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.payload_id)
        .collect();
    assert_eq!(ids, vec!["payload-raw".to_string()]);
}

/// `option<string>` fields reject an explicit JSON null: an unlinked reading and a
/// verse-less payload must store (and publish) with the field ABSENT. Before the
/// fix every reading without a `payload_id` failed its insert.
#[tokio::test]
async fn unlinked_reading_and_verseless_payload_store_with_option_fields_absent() {
    let db = setup_db().await;
    let (store, handle) = blob_store();
    seed_hierarchy(&db, VERSE, FRACTAL, PETAL, "node-a").await;
    declare_format(&db, VERSE, "acme.thermo/frame@1", FORMAT_MIME).await;
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);

    let written = insert_readings_with_replication(
        &db,
        PETAL,
        Some(VERSE),
        "did:key:z6MkSensor",
        SourceClass::Device,
        &[IotReadingInput {
            node_id: "node-a".to_string(),
            metric: "temperature_c".to_string(),
            value: 21.5,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: None,
        }],
        Some(&handle),
        Some(&repl_tx),
    )
    .await
    .expect("an unlinked reading is accepted");
    assert_eq!(written, 1);
    let event = repl_rx.try_recv().expect("the reading is published");
    let published: serde_json::Value =
        serde_json::from_slice(&store.bytes_for(&event.content_hash).unwrap()).unwrap();
    assert!(published.get("payload_id").is_none(), "{published}");
    let mut res = db
        .query("SELECT * FROM iot_reading WHERE payload_id = NONE")
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), 1, "stored with payload_id NONE");

    let asset_id = seed_asset(&db, &handle, b"frame", PETAL, FORMAT_MIME).await;
    let (payload_id, _) = insert_device_payloads(
        &db,
        PETAL,
        None,
        "did:key:z6MkEditor",
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: "node-a".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            asset_ids: vec![asset_id],
            recorded_at: None,
            payload_id: None,
        },
        None,
        None,
    )
    .await
    .expect("a verse-less payload is accepted");
    let listed = list_device_payloads(&db, PETAL, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].payload_id, payload_id);
    assert_eq!(listed[0].verse_id, None);
}
