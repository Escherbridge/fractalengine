//! Petal snapshot export then import through the DB-thread handlers; see `src/handlers/AGENTS.md` §petal-snapshot.

use std::time::{SystemTime, UNIX_EPOCH};

use fe_database::handlers::petal_snapshot::{export_petal_snapshot, import_petal_snapshot, ImportError};
use fe_database::op_log::commit_operation;
use fe_database::types::{NodeId, OpLogEntry, OpType};
use serde_json::{json, Value};

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

const VERSE: &str = "verse-snap";
const FRACTAL: &str = "fractal-snap";
/// The exported petal.
const PETAL: &str = "petal-a";
/// A sibling whose id extends the exported one, so a bare substring match would leak its rows.
const SIBLING: &str = "petal-a2";

fn petal_scope(petal_id: &str) -> String {
    format!("VERSE#{VERSE}-FRACTAL#{FRACTAL}-PETAL#{petal_id}")
}

/// First DB of the journey: resets the process HLC, then opens a schema'd in-memory DB.
async fn setup_db() -> Db {
    fe_database::op_log::init_hlc(0);
    fresh_db().await
}

/// Another schema'd in-memory DB that leaves the process HLC untouched.
async fn fresh_db() -> Db {
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("apply schema");
    db
}

async fn run(db: &Db, sql: &'static str, bindings: Value) {
    let mut query = db.query(sql);
    if let Value::Object(map) = bindings {
        for (name, value) in map {
            query = query.bind((name, value));
        }
    }
    query.await.expect(sql).check().expect(sql);
}

async fn seed_node(db: &Db, petal_id: &str, node_id: &str, asset_id: Option<&str>) {
    let sql = if asset_id.is_some() {
        "CREATE node CONTENT { node_id: $nid, petal_id: $pid, asset_id: $aid, \
         position: <geometry<point>> [1.0, 2.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
         scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-11-13T00:00:00Z' }"
    } else {
        "CREATE node CONTENT { node_id: $nid, petal_id: $pid, \
         position: <geometry<point>> [1.0, 2.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
         scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-11-13T00:00:00Z' }"
    };
    run(db, sql, json!({"nid": node_id, "pid": petal_id, "aid": asset_id})).await;
}

async fn seed_op(db: &Db, node_id: &str, op_type: OpType, payload: Value) {
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type,
        payload,
        sig: "00".repeat(64),
    };
    commit_operation(db, entry, |_| async { Ok::<(), anyhow::Error>(()) })
        .await
        .expect("seed op-log entry");
}

/// Two petals, a verse-level role and decoy rows; only the exported petal's state may come back.
async fn seed_two_petals(db: &Db) {
    seed_fractal(db).await;
    for petal_id in [PETAL, SIBLING] {
        run(db, "CREATE petal CONTENT { petal_id: $pid, name: $pid, node_id: 'did:key:owner', created_at: '2026-11-13T00:00:00Z', visibility: 'private', tags: [], fractal_id: $fid }",
            json!({"pid": petal_id, "fid": FRACTAL})).await;
    }
    seed_node(db, PETAL, "node-a1", Some("asset-node")).await;
    seed_node(db, PETAL, "node-a2", None).await;
    seed_node(db, SIBLING, "node-b1", Some("asset-sibling")).await;
    for (log_id, node_id) in [("log-a1", "node-a1"), ("log-b1", "node-b1")] {
        run(db, "CREATE node_log CONTENT { log_id: $lid, node_id: $nid, hlc_timestamp: 1, op: 'baseline', payload: {}, row_version: 1, created_at: '2026-11-13T00:00:00Z' }",
            json!({"lid": log_id, "nid": node_id})).await;
    }
    for (field_def_id, scope) in [("fd-petal", petal_scope(PETAL)), ("fd-sibling", petal_scope(SIBLING)), ("fd-global", "global".to_string())] {
        run(db, "CREATE field_def CONTENT { field_def_id: $id, scope: $scope, entity_type: 'node', key: $id, value_type: 'string', created_by: 'did:key:owner', created_at: '2026-11-13T00:00:00Z' }",
            json!({"id": field_def_id, "scope": scope})).await;
    }
    run(db, "CREATE room CONTENT { petal_id: $pid, name: 'Lobby' }", json!({"pid": PETAL})).await;
    run(db, "CREATE model CONTENT { petal_id: $pid, asset_id: 'asset-model', transform: {}, tags: [] }", json!({"pid": PETAL})).await;
    for asset_id in ["asset-node", "asset-model", "asset-sibling", "asset-unplaced"] {
        run(db, "CREATE asset CONTENT { asset_id: $aid, name: $aid, content_type: 'model/gltf-binary', size_bytes: 4, created_at: '2026-11-13T00:00:00Z' }",
            json!({"aid": asset_id})).await;
    }
    for (peer_did, scope) in [
        ("did:key:petal-editor", petal_scope(PETAL)),
        ("did:key:verse-manager", format!("VERSE#{VERSE}")),
        ("did:key:sibling-editor", petal_scope(SIBLING)),
    ] {
        run(db, "CREATE role CONTENT { peer_did: $peer, scope: $scope, role: 'editor' }", json!({"peer": peer_did, "scope": scope})).await;
    }
    for (reading_id, node_id, petal_id) in [("reading-a", "node-a1", PETAL), ("reading-b", "node-b1", SIBLING)] {
        run(db, "CREATE iot_reading CONTENT { reading_id: $rid, node_id: $nid, petal_id: $pid, metric: 'temp', value: 21.5, recorded_at: '2026-11-13T00:00:00Z', recorded_at_ms: 1, hlc_timestamp: 1 }",
            json!({"rid": reading_id, "nid": node_id, "pid": petal_id})).await;
    }

    // Op-log: three entries attributable to PETAL by each predicate clause, four that are not.
    seed_op(db, "did:key:owner", OpType::CreatePetal, json!({"petal_id": PETAL, "name": PETAL})).await;
    seed_op(db, "node-a1", OpType::TransformUpdate, json!({"node_id": "node-a1", "position": [0.0, 0.0, 0.0]})).await;
    seed_op(db, "did:key:owner", OpType::AssignRole, json!({"peer_did": "did:key:petal-editor", "scope": petal_scope(PETAL), "role": "editor"})).await;
    seed_op(db, SIBLING, OpType::SetPetalTerrain, json!({"petal_id": SIBLING, "terrain": null})).await;
    seed_op(db, "did:key:owner", OpType::AssignRole, json!({"peer_did": "did:key:sibling-editor", "scope": petal_scope(SIBLING), "role": "editor"})).await;
    seed_op(db, "node-b1", OpType::PropertySet, json!({"node_id": "node-b1", "key": "k", "value": 1})).await;
    seed_op(db, "did:key:owner", OpType::ElevatedQuery, json!({"sql": "SELECT 1", "actor_did": "did:key:owner"})).await;
}

async fn seed_fractal(db: &Db) {
    run(db, "CREATE fractal CONTENT { fractal_id: $fid, verse_id: $vid, owner_did: 'did:key:owner', name: 'F', created_at: '2026-11-13T00:00:00Z' }",
        json!({"fid": FRACTAL, "vid": VERSE})).await;
}

async fn select(db: &Db, sql: &'static str) -> Vec<Value> {
    let mut response = db.query(sql).await.expect(sql).check().expect(sql);
    response.take(0).expect(sql)
}

fn lamports(rows: &[Value]) -> Vec<u64> {
    rows.iter()
        .map(|row| row["lamport_clock"].as_u64().expect("lamport_clock"))
        .collect()
}

#[tokio::test]
async fn petal_snapshot_export_then_import_journey() {
    let db = setup_db().await;
    seed_two_petals(&db).await;

    // Export leg.
    let snapshot = export_petal_snapshot(&db, PETAL).await.expect("export");
    assert_eq!(snapshot.petal["petal_id"], json!(PETAL));
    let counts = snapshot.table_row_counts();
    let expected = [
        ("petal", 1), ("nodes", 2), ("node_log", 1), ("field_defs", 1), ("rooms", 1),
        ("models", 1), ("assets", 2), ("roles", 1), ("iot_readings", 1), ("oplog", 3),
    ];
    for (table, count) in expected {
        assert_eq!(counts[table], count, "{table} row count");
    }
    assert_eq!(snapshot.roles[0]["peer_did"], json!("did:key:petal-editor"), "inherited and sibling roles excluded");
    let asset_ids: Vec<&str> = snapshot.assets.iter().filter_map(|a| a["asset_id"].as_str()).collect();
    assert_eq!(asset_ids, ["asset-model", "asset-node"], "only assets the petal's nodes and models reference");
    assert!(snapshot.nodes.iter().all(|node| node.get("id").is_some()), "rows are verbatim, record ids included");

    let exported_lamports = lamports(&snapshot.oplog);
    assert!(exported_lamports.windows(2).all(|pair| pair[0] < pair[1]), "op-log strictly lamport-ascending: {exported_lamports:?}");
    let op_types: Vec<&str> = snapshot.oplog.iter().filter_map(|e| e["op_type"].as_str()).collect();
    assert_eq!(op_types, ["CreatePetal", "TransformUpdate", "AssignRole"]);

    // The export is audited after collection: one ExportPetal entry, newer than every exported entry.
    let mut audit = db
        .query("SELECT * FROM op_log WHERE op_type = 'ExportPetal' AND payload.petal_id = $pid")
        .bind(("pid", PETAL.to_string()))
        .await
        .expect("read export audit");
    let audit: Vec<Value> = audit.take(0).expect("audit rows");
    assert_eq!(audit.len(), 1, "one ExportPetal entry per export");
    assert!(lamports(&audit)[0] > *exported_lamports.last().unwrap());

    // Exporting is a read: a second export returns the same state rows plus the first export's audit entry.
    let again = export_petal_snapshot(&db, PETAL).await.expect("second export");
    assert_eq!(again.nodes, snapshot.nodes);
    assert_eq!(again.iot_readings, snapshot.iot_readings);
    assert_eq!(again.oplog.len(), snapshot.oplog.len() + 1);

    assert!(export_petal_snapshot(&db, "petal-missing").await.is_err(), "an unknown petal is refused");

    // Import leg: restore into a fresh DB whose hierarchy already holds the parent fractal.
    let restored_db = fresh_db().await;
    seed_fractal(&restored_db).await;
    import_petal_snapshot(&restored_db, snapshot.clone()).await.expect("import into a fresh DB");
    let restored = export_petal_snapshot(&restored_db, PETAL).await.expect("export the restored petal");
    assert_eq!(restored.petal, snapshot.petal, "petal row restored verbatim");
    assert_eq!(restored.nodes, snapshot.nodes, "node rows restored verbatim, record ids and geometry included");
    assert_eq!(restored.node_log, snapshot.node_log);
    assert_eq!(restored.field_defs, snapshot.field_defs);
    assert_eq!(restored.rooms, snapshot.rooms);
    assert_eq!(restored.models, snapshot.models);
    assert_eq!(restored.assets, snapshot.assets);
    assert_eq!(restored.iot_readings, snapshot.iot_readings);
    assert!(restored.roles.is_empty(), "role rows are a materialized cache of signed statements and are never restored");
    let imported = snapshot.oplog.len();
    assert_eq!(&restored.oplog[..imported], &snapshot.oplog[..], "op-log restored verbatim, same count and order");
    let audit_types: Vec<&str> = restored.oplog[imported..].iter().filter_map(|e| e["op_type"].as_str()).collect();
    assert_eq!(audit_types, ["ImportPetal"], "the import is audited once, after the restored entries");
    assert!(lamports(&restored.oplog[imported..])[0] > *exported_lamports.last().unwrap());
    let again = import_petal_snapshot(&restored_db, snapshot.clone()).await;
    assert!(matches!(again, Err(ImportError::PetalExists)), "a second import is refused: {again:?}");

    // HLC leg: an imported clock just inside the five-minute future bound must order every later write after it.
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_millis() as u64;
    let far_wall = now_ms + 240_000;
    let far_lamport = (far_wall << 16) | 7;
    let mut ahead = snapshot.clone();
    ahead.oplog.push(json!({
        "id": "op_log:far_future", "lamport_clock": far_lamport, "hlc_timestamp": format!("{far_wall}:0007"),
        "node_id": "node-a1", "op_type": "TransformUpdate", "payload": {"node_id": "node-a1"}, "sig": "00".repeat(64),
    }));
    let clock_db = fresh_db().await;
    run(&clock_db, "CREATE asset CONTENT { asset_id: 'asset-node', name: 'local', content_type: 'model/gltf-binary', size_bytes: 4, created_at: '2026-11-13T00:00:00Z' }",
        json!({})).await;
    import_petal_snapshot(&clock_db, ahead).await.expect("import carrying a near-future clock");
    let audit_lamport = select(&clock_db, "SELECT VALUE lamport_clock FROM op_log WHERE op_type = 'ImportPetal'").await;
    assert!(audit_lamport[0].as_u64().unwrap() > far_lamport, "the ImportPetal entry is stamped after the HLC advance");
    let write = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId("node-a1".to_string()),
        op_type: OpType::TransformUpdate,
        payload: json!({"node_id": "node-a1", "position": [3.0, 0.0, 3.0]}),
        sig: "00".repeat(64),
    };
    let next = commit_operation(&clock_db, write, |lamport| async move { Ok::<u64, anyhow::Error>(lamport) })
        .await
        .expect("first write after import");
    assert!(next > far_lamport, "post-import write {next} must order after the imported max {far_lamport}");
    let local_asset = select(&clock_db, "SELECT VALUE name FROM asset WHERE asset_id = 'asset-node'").await;
    assert_eq!(local_asset, [json!("local")], "an existing asset with the same id is kept, not duplicated");

    // Refusal leg: rows outside the petal are refused up front; a SCHEMAFULL rejection mid-transaction leaves nothing.
    let refused_db = fresh_db().await;
    let mut foreign_node = snapshot.clone();
    foreign_node.nodes[0]["petal_id"] = json!(SIBLING);
    let mut unreferenced_asset = snapshot.clone();
    unreferenced_asset.assets[0]["asset_id"] = json!("asset-unplaced");
    for (case, rows) in [("foreign node", foreign_node), ("unreferenced asset", unreferenced_asset)] {
        let outcome = import_petal_snapshot(&refused_db, rows).await;
        assert!(matches!(outcome, Err(ImportError::Invalid(_))), "{case}: {outcome:?}");
    }
    let mut corrupted = snapshot.clone();
    corrupted.nodes[1]["elevation"] = json!("high");
    let outcome = import_petal_snapshot(&refused_db, corrupted).await;
    assert!(matches!(outcome, Err(ImportError::Db(_))), "the schema rejects the corrupted node: {outcome:?}");
    for sql in ["SELECT * FROM petal", "SELECT * FROM node", "SELECT * FROM node_log", "SELECT * FROM asset", "SELECT * FROM op_log"] {
        assert!(select(&refused_db, sql).await.is_empty(), "{sql}: no partial petal survives a refused import");
    }
}
