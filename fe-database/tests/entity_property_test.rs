//! Regression test for the annotation-save silent-failure bug: SetNodeProperty
//! against a nonexistent node_id must surface as `DbResult::Error`, not silently
//! succeed. See `fe-database/src/handlers/entity_property.rs` (matched-row assertion).

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::sync::Arc;
use std::time::Duration;

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

struct TestDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    _tmp_dir: tempfile::TempDir,
}

fn spawn_test_db() -> TestDb {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
    let db_path = tmp_dir
        .path()
        .join("entity_property_test.db")
        .to_string_lossy()
        .to_string();

    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(256);

    let _handle = fe_database::spawn_db_thread_with_sync(
        cmd_rx,
        res_tx,
        mock_blob_store(),
        None,
        None,
        None,
        None,
        Some(db_path),
    )
    .expect("entity_property test DB init failed");

    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("entity_property test DB did not start within 30s");
    assert!(
        matches!(started, DbResult::Started),
        "expected DbResult::Started, got {started:?}"
    );

    TestDb {
        cmd_tx,
        res_rx,
        _tmp_dir: tmp_dir,
    }
}

#[test]
fn set_node_property_on_nonexistent_node_returns_error() {
    let db = spawn_test_db();

    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: "does-not-exist".to_string(),
            key: "gis.annotation.title".to_string(),
            value: serde_json::json!("test"),
            correlation_id: None,
        })
        .unwrap();

    let res = db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("SetNodeProperty result");
    match res {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("matched no node"),
                "expected 'matched no node' error, got: {msg}"
            );
        }
        other => panic!("expected DbResult::Error for nonexistent node_id, got {other:?}"),
    }
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: "does-not-exist".into(),
            key: "gis.annotation.title".into(),
            value: serde_json::json!("test"),
            correlation_id: Some("api-property-failure".into()),
        })
        .unwrap();
    assert!(matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::RequestFailed { correlation_id, error }
        if correlation_id == "api-property-failure" && error.contains("matched no node")));
}

// --- HS-P6-CALLOUT: a dimension_callout node's property bag round-trips ---

/// Build the minimal verse -> fractal -> petal hierarchy and return the petal ID.
fn seed_hierarchy(db: &TestDb) -> String {
    db.cmd_tx
        .send(DbCommand::CreateVerse {
            name: "callout-test-verse".to_string(),
        })
        .unwrap();
    let verse_id = match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateVerse result")
    {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreateFractal {
            verse_id,
            name: "callout-test-fractal".to_string(),
        })
        .unwrap();
    let fractal_id = match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateFractal result")
    {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreatePetal {
            fractal_id,
            name: "callout-test-petal".to_string(),
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreatePetal result")
    {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node in `petal_id` at local `position` and return its node ID.
fn create_node(db: &TestDb, petal_id: &str, name: &str, position: [f32; 3]) -> String {
    db.cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
            position,
            correlation_id: None,
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateNode result")
    {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    }
}

/// Set a custom property on a node via the real `SetNodeProperty` command.
fn set_property(db: &TestDb, node_id: &str, key: &str, value: serde_json::Value) {
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
            correlation_id: None,
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("SetNodeProperty result")
    {
        DbResult::NodePropertySet { .. } => {}
        other => panic!("expected NodePropertySet, got {other:?}"),
    }
}

/// Load a node's custom properties via the real `GetNodeProperties` command.
fn get_properties(db: &TestDb, node_id: &str) -> serde_json::Value {
    db.cmd_tx
        .send(DbCommand::GetNodeProperties {
            node_id: node_id.to_string(),
            correlation_id: None,
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("GetNodeProperties result")
    {
        DbResult::NodePropertiesLoaded { properties, .. } => properties,
        other => panic!("expected NodePropertiesLoaded, got {other:?}"),
    }
}

/// Hard-delete a node via the real `DeleteNode` command.
fn delete_node(db: &TestDb, node_id: &str) {
    db.cmd_tx
        .send(DbCommand::DeleteNode {
            node_id: node_id.to_string(),
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("DeleteNode result")
    {
        DbResult::NodeDeleted { .. } => {}
        other => panic!("expected NodeDeleted, got {other:?}"),
    }
}

/// Reload the whole hierarchy and report whether `node_id` is present anywhere in it.
fn hierarchy_contains_node(db: &TestDb, node_id: &str) -> bool {
    db.cmd_tx.send(DbCommand::LoadHierarchy).unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("LoadHierarchy result")
    {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .flat_map(|p| &p.nodes)
            .any(|n| n.id == node_id),
        other => panic!("expected HierarchyLoaded, got {other:?}"),
    }
}

#[test]
fn a_dimension_callout_node_round_trips_through_the_property_store() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Dimension 5.00 m", [0.0, 0.0, 0.0]);

    let anchors = serde_json::json!([[0.0, 0.0, 0.0], [3.0, 0.0, 4.0],]);
    set_property(
        &db,
        &node_id,
        "node_kind",
        serde_json::json!("dimension_callout"),
    );
    set_property(&db, &node_id, "dimension.mode", serde_json::json!("tape"));
    set_property(&db, &node_id, "dimension.anchors", anchors.clone());
    set_property(&db, &node_id, "dimension.closed", serde_json::json!(false));

    let props = get_properties(&db, &node_id);
    assert_eq!(props["node_kind"], serde_json::json!("dimension_callout"));
    assert_eq!(props["dimension.mode"], serde_json::json!("tape"));
    assert_eq!(
        props["dimension.anchors"], anchors,
        "f64 anchors must round-trip exactly"
    );
    assert_eq!(props["dimension.closed"], serde_json::json!(false));
    let exact_integer = 9_007_199_254_740_993_i64;
    set_property(
        &db,
        &node_id,
        "precision",
        serde_json::json!({"counter":exact_integer}),
    );
    assert_eq!(
        get_properties(&db, &node_id)["precision"]["counter"].as_i64(),
        Some(exact_integer)
    );
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.clone(),
            key: "precision".into(),
            value: serde_json::json!({"counter":u64::MAX}),
            correlation_id: Some("overflow-property".into()),
        })
        .unwrap();
    assert!(
        matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),DbResult::RequestFailed {error,..} if error.contains("signed 64-bit"))
    );
    assert_eq!(
        get_properties(&db, &node_id)["precision"]["counter"].as_i64(),
        Some(exact_integer)
    );

    db.cmd_tx
        .send(DbCommand::ResolveNodeScope {
            node_id: node_id.clone(),
        })
        .unwrap();
    let scope = match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
        DbResult::ScopeResolved { scope: Some(scope) } => scope,
        other => panic!("expected resolved scope, got {other:?}"),
    };
    db.cmd_tx
        .send(DbCommand::CreateFieldDef {
            scope: scope.clone(),
            entity_type: "node".into(),
            key: "dimension.anchors".into(),
            value_type: "array".into(),
            default_val: None,
        })
        .unwrap();
    let field_def_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
        DbResult::FieldDefCreated { field_def_id, .. } => field_def_id,
        other => panic!("expected definition, got {other:?}"),
    };
    db.cmd_tx
        .send(DbCommand::CreateFieldDef {
            scope: scope.clone(),
            entity_type: "node".into(),
            key: "precision".into(),
            value_type: "object".into(),
            default_val: Some(serde_json::json!({"counter":exact_integer})),
        })
        .unwrap();
    assert!(matches!(
        db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::FieldDefCreated { .. }
    ));
    db.cmd_tx
        .send(DbCommand::ListFieldDefs {
            scope: scope.clone(),
        })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
        DbResult::FieldDefsListed { field_defs, .. } => assert_eq!(
            field_defs
                .iter()
                .find(|f| f.key == "precision")
                .unwrap()
                .default_val
                .as_ref()
                .unwrap()["counter"]
                .as_i64(),
            Some(exact_integer)
        ),
        other => panic!("precision default readback: {other:?}"),
    }
    db.cmd_tx
        .send(DbCommand::CreateFieldDef {
            scope,
            entity_type: "node".into(),
            key: "overflow".into(),
            value_type: "object".into(),
            default_val: Some(serde_json::json!({"counter":u64::MAX})),
        })
        .unwrap();
    assert!(
        matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),DbResult::Error(error) if error.contains("signed 64-bit"))
    );
    let descriptor = serde_json::json!({"version":1, "schema":{
        "type":"array", "minItems":2, "items":{"type":"array", "minItems":3,
            "maxItems":3, "items":{"type":"number"}}
    }});
    let parsed = fe_sdk::property_schema::EnforcedPropertySchema::parse(&descriptor).unwrap();
    let exact_bound = fe_sdk::property_schema::EnforcedPropertySchema::parse(
        &serde_json::json!({"version":1,"schema":{"type":"integer","maximum":9007199254740992_u64}})).unwrap();
    assert!(exact_bound
        .validate(&serde_json::json!(9007199254740993_u64))
        .is_err());
    for schema in [
        serde_json::json!({"type":"string","items":null}),
        serde_json::json!({"type":"number","maximum":null}),
    ] {
        assert!(fe_sdk::property_schema::EnforcedPropertySchema::parse(
            &serde_json::json!({"version":1,"schema":schema})
        )
        .is_err());
    }
    assert!(parsed.validate(&anchors).is_ok());
    assert!(parsed
        .validate(&serde_json::json!("[[0,0,0],[3,0,4]]"))
        .is_err());
    let mut unsupported = descriptor.clone();
    unsupported["schema"]["$ref"] = serde_json::json!("https://example.invalid/schema");
    assert!(fe_sdk::property_schema::EnforcedPropertySchema::parse(&unsupported).is_err());
    let unconstrained = fe_sdk::property_schema::EnforcedPropertySchema::parse(
        &serde_json::json!({"version":1,"schema":{"type":"object"}}),
    )
    .unwrap();
    let mut deep = serde_json::json!(0);
    for _ in 0..17 {
        deep = serde_json::json!({"child":deep});
    }
    assert!(
        unconstrained.validate(&deep).is_err(),
        "additional members still obey the depth bound"
    );
    db.cmd_tx
        .send(DbCommand::SetFieldSchema {
            field_def_id: field_def_id.clone(),
            descriptor: descriptor.clone(),
            correlation_id: Some("schema-v1".into()),
        })
        .unwrap();
    assert!(matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::QueryResult { correlation_id: Some(id), .. } if id == "schema-v1"));
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.clone(),
            key: "dimension.anchors".into(),
            value: serde_json::json!("wrong type"),
            correlation_id: Some("invalid-property".into()),
        })
        .unwrap();
    assert!(matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::RequestFailed { correlation_id, .. } if correlation_id == "invalid-property"));
    assert_eq!(get_properties(&db, &node_id)["dimension.anchors"], anchors);
    let mut incompatible = descriptor.clone();
    incompatible["version"] = serde_json::json!(2);
    incompatible["schema"]["maxItems"] = serde_json::json!(1);
    db.cmd_tx
        .send(DbCommand::SetFieldSchema {
            field_def_id: field_def_id.clone(),
            descriptor: incompatible,
            correlation_id: Some("schema-incompatible".into()),
        })
        .unwrap();
    assert!(matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::RequestFailed { correlation_id, .. } if correlation_id == "schema-incompatible"));
    db.cmd_tx
        .send(DbCommand::DeleteFieldDef {
            field_def_id: field_def_id.clone(),
        })
        .unwrap();
    assert!(matches!(
        db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::Error(_)
    ));

    let historical = create_node(&db, &petal_id, "Historical calibration", [0.0; 3]);
    let calibration_v1 = serde_json::json!({"gain":2,"axes":["x","z"],"sensor":{"serial":"ALPHA-01","range":[-40,85]}});
    set_property(&db, &historical, "calibration", calibration_v1.clone());
    for (position, expected_version) in [([3.0, 0.0, 4.0], 3), ([6.0, 0.0, 8.0], 5)] {
        db.cmd_tx
            .send(DbCommand::CommitNodeMutation {
                node_id: historical.clone(),
                actor_did: "did:qa-editor".into(),
                correlation_id: Some(format!("move-{expected_version}")),
                mutation: fe_runtime::messages::NodeMutation::FullTransform {
                    position,
                    rotation: [0.0; 3],
                    scale: [1.0; 3],
                },
            })
            .unwrap();
        match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
            DbResult::NodeMutationCommitted { receipt, .. } => {
                let data = [serde_json::to_value(receipt).unwrap()];
                assert_eq!(data[0]["history_version"], expected_version);
                assert_eq!(data[0]["state"]["position"], serde_json::json!(position));
            }
            other => panic!("committed transform: {other:?}"),
        }
        if expected_version == 3 {
            let mut updated = calibration_v1.clone();
            updated["gain"] = serde_json::json!(3);
            set_property(&db, &historical, "calibration", updated);
        }
    }
    for (after, expected, next) in [(1, vec![2, 3], Some(3)), (3, vec![4, 5], None)] {
        db.cmd_tx
            .send(DbCommand::ReadNodeHistory {
                node_id: historical.clone(),
                after_version: after,
                limit: 2,
                at_version: None,
                correlation_id: Some(format!("page-{after}")),
            })
            .unwrap();
        match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
            DbResult::NodeHistoryLoaded { page, .. } => {
                let data = [serde_json::to_value(page).unwrap()];
                let versions: Vec<i64> = data[0]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["row_version"].as_i64().unwrap())
                    .collect();
                assert_eq!(versions, expected);
                assert_eq!(data[0]["next_after_version"].as_i64(), next);
                assert_eq!(data[0]["coverage_start_version"], 1);
                assert_eq!(data[0]["current_state_verified"], true);
            }
            other => panic!("history page: {other:?}"),
        }
    }
    for (version, gain, position) in [
        (3, 2, [3.0, 0.0, 4.0]),
        (4, 3, [3.0, 0.0, 4.0]),
        (5, 3, [6.0, 0.0, 8.0]),
    ] {
        db.cmd_tx
            .send(DbCommand::ReadNodeHistory {
                node_id: historical.clone(),
                after_version: 0,
                limit: 1,
                at_version: Some(version),
                correlation_id: Some(format!("state-{version}")),
            })
            .unwrap();
        match db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap() {
            DbResult::NodeStateAtVersionLoaded { state, .. } => {
                let data = [serde_json::to_value(state).unwrap()];
                assert_eq!(data[0]["availability"], "available");
                assert_eq!(data[0]["state"]["properties"]["calibration"]["gain"], gain);
                assert_eq!(data[0]["state"]["position"], serde_json::json!(position));
            }
            other => panic!("historical state: {other:?}"),
        }
    }
    db.cmd_tx
        .send(DbCommand::UpdateFieldDef {
            field_def_id,
            value_type: "string".into(),
            default_val: None,
        })
        .unwrap();
    assert!(matches!(
        db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::Error(_)
    ));

    assert!(
        hierarchy_contains_node(&db, &node_id),
        "the live node is present before delete"
    );
    delete_node(&db, &node_id);
    assert!(
        !hierarchy_contains_node(&db, &node_id),
        "a deleted callout node is absent from a live hierarchy load"
    );
}

#[test]
fn delete_node_property_on_nonexistent_node_returns_error() {
    let db = spawn_test_db();

    db.cmd_tx
        .send(DbCommand::DeleteNodeProperty {
            node_id: "does-not-exist".to_string(),
            key: "gis.annotation.title".to_string(),
            correlation_id: None,
        })
        .unwrap();

    let res = db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("DeleteNodeProperty result");
    match res {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("matched no node"),
                "expected 'matched no node' error, got: {msg}"
            );
        }
        other => panic!("expected DbResult::Error for nonexistent node_id, got {other:?}"),
    }
    db.cmd_tx
        .send(DbCommand::DeleteNodeProperty {
            node_id: "does-not-exist".into(),
            key: "gis.annotation.title".into(),
            correlation_id: Some("api-property-failure".into()),
        })
        .unwrap();
    assert!(matches!(db.res_rx.recv_timeout(CMD_TIMEOUT).unwrap(),
        DbResult::RequestFailed { correlation_id, error }
        if correlation_id == "api-property-failure" && error.contains("matched no node")));
}
