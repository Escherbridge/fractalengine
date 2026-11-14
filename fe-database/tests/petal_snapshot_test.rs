//! Petal snapshot export then import through the DB-thread handlers; see `src/handlers/AGENTS.md` §petal-snapshot.

use std::time::{SystemTime, UNIX_EPOCH};

use fe_database::handlers::petal_snapshot::{
    export_petal_snapshot, import_petal_snapshot, import_petal_snapshot_reply, ImportError,
};
use fe_database::op_log::commit_operation;
use fe_database::types::{NodeId, OpLogEntry, OpType};
use fe_format::snapshot as archive_format;
use fe_format::{AssetEntry, EntryKind, HexonManifest, HexonType};
use fe_runtime::messages::DbResult;
use fe_runtime::petal_snapshot::PetalSnapshotRows;
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

/// Rows to the plain-JSON section type fe-format archives.
fn to_data(rows: &PetalSnapshotRows) -> archive_format::PetalSnapshotData {
    archive_format::PetalSnapshotData {
        petal: rows.petal.clone(),
        nodes: rows.nodes.clone(),
        node_log: rows.node_log.clone(),
        field_defs: rows.field_defs.clone(),
        rooms: rows.rooms.clone(),
        models: rows.models.clone(),
        assets: rows.assets.clone(),
        roles: rows.roles.clone(),
        iot_readings: rows.iot_readings.clone(),
        oplog: rows.oplog.clone(),
    }
}

/// The archive section back to the rows the import handler takes.
fn to_rows(data: archive_format::PetalSnapshotData) -> PetalSnapshotRows {
    PetalSnapshotRows {
        petal: data.petal,
        nodes: data.nodes,
        node_log: data.node_log,
        field_defs: data.field_defs,
        rooms: data.rooms,
        models: data.models,
        assets: data.assets,
        roles: data.roles,
        iot_readings: data.iot_readings,
        oplog: data.oplog,
    }
}

/// One synthetic content-addressed blob per asset row, sorted by hash.
fn blobs_for(rows: &PetalSnapshotRows) -> Vec<(String, Vec<u8>)> {
    let mut blobs: Vec<(String, Vec<u8>)> = rows
        .assets
        .iter()
        .filter_map(|asset| asset["asset_id"].as_str())
        .map(|asset_id| {
            let bytes = format!("blob-of-{asset_id}").into_bytes();
            (blake3::hash(&bytes).to_hex().to_string(), bytes)
        })
        .collect();
    blobs.sort();
    blobs
}

/// Build the PetalSnapshot archive; `stamp` and `build_id` feed only the volatile fields.
fn build_archive(rows: &PetalSnapshotRows, stamp: &str, build_id: Option<&str>) -> Vec<u8> {
    let blobs = blobs_for(rows);
    let entries: Vec<AssetEntry> = blobs
        .iter()
        .map(|(hash, _)| AssetEntry {
            entry_id: format!("entry-{}", &hash[..8]),
            kind: EntryKind::Model,
            asset_hash: hash.clone(),
            format: "bin".into(),
            label: "snapshot blob".into(),
            tags: vec![],
            description: None,
            is_placeable: false,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        })
        .collect();
    let manifest = HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: format!("petal-{PETAL}"),
        hexon_type: HexonType::PetalSnapshot,
        publisher_did: "did:key:owner".into(),
        publisher_name: None,
        version: "1.0.0".into(),
        build_id: build_id.map(str::to_string),
        name: PETAL.into(),
        description: None,
        tags: vec![],
        created_at: stamp.into(),
        updated_at: stamp.into(),
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        integrity: None,
        signature: None,
    };
    let lamport = |row: &Value| row["lamport_clock"].as_u64().expect("lamport_clock");
    let op_log_range = match (rows.oplog.first(), rows.oplog.last()) {
        (Some(first), Some(last)) => archive_format::OpLogRange {
            min_lamport: lamport(first),
            max_lamport: lamport(last),
            count: rows.oplog.len() as u64,
        },
        _ => archive_format::OpLogRange::default(),
    };
    let meta = archive_format::SnapshotMeta {
        snapshot_format_version: archive_format::SNAPSHOT_FORMAT_VERSION,
        oplog_predicate_version: archive_format::OPLOG_PREDICATE_VERSION,
        petal_id: PETAL.into(),
        scope: petal_scope(PETAL),
        source_node_id: "did:key:owner".into(),
        exported_at: stamp.into(),
        table_row_counts: rows
            .table_row_counts()
            .into_iter()
            .map(|(table, count)| (table, count as u64))
            .collect(),
        op_log_range,
    };
    archive_format::export_petal_snapshot(manifest, &meta, &to_data(rows), entries, blobs)
        .expect("build snapshot archive")
}

/// State tables as order-insensitive row sets (roles excluded: never restored).
fn state_row_sets(rows: &PetalSnapshotRows) -> Vec<(&'static str, Vec<String>)> {
    let sorted = |table: &[Value]| {
        let mut rendered: Vec<String> = table.iter().map(Value::to_string).collect();
        rendered.sort();
        rendered
    };
    vec![
        ("petal", vec![rows.petal.to_string()]),
        ("nodes", sorted(&rows.nodes)),
        ("node_log", sorted(&rows.node_log)),
        ("field_defs", sorted(&rows.field_defs)),
        ("rooms", sorted(&rows.rooms)),
        ("models", sorted(&rows.models)),
        ("assets", sorted(&rows.assets)),
        ("iot_readings", sorted(&rows.iot_readings)),
    ]
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
        ("models", 1), ("assets", 2), ("roles", 1), ("iot_readings", 1), ("oplog", 2),
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
    assert_eq!(op_types, ["CreatePetal", "TransformUpdate"], "role statements are not data operations and are never exported");

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

    // FR-9 leg: rows -> archive -> rows -> fresh DB -> re-export is a fixed point (volatile stamps excluded).
    let a_rows = export_petal_snapshot(&db, PETAL).await.expect("export for the archive loop");
    let a_archive = build_archive(&a_rows, "2026-11-13T00:00:00Z", None);
    assert_eq!(a_archive, build_archive(&a_rows, "2026-11-13T00:00:00Z", None), "same rows and stamps build byte-identical archives");
    let read_back = archive_format::import_petal_snapshot(&a_archive).expect("archive imports");
    assert_eq!(read_back.data, to_data(&a_rows), "the archive carries every row verbatim, op-log order included");
    assert_eq!(read_back.meta.table_row_counts.get("oplog"), Some(&(a_rows.oplog.len() as u64)));
    assert_eq!(read_back.blobs, blobs_for(&a_rows), "blobs come back sorted and intact");
    assert!(read_back.blobs.iter().all(|(name, body)| blake3::hash(body).to_hex().as_str() == name), "every blob name re-hashes");
    assert!(read_back.entries.iter().all(|entry| read_back.blobs.iter().any(|(name, _)| *name == entry.asset_hash)), "every catalog entry points at a shipped blob");

    let c_db = fresh_db().await;
    seed_fractal(&c_db).await;
    import_petal_snapshot(&c_db, to_rows(read_back.data.clone())).await.expect("import the archive's rows into a fresh DB");
    let c_rows = export_petal_snapshot(&c_db, PETAL).await.expect("re-export from the fresh DB");
    assert_eq!(state_row_sets(&c_rows), state_row_sets(&a_rows), "every state table equal as a row set");
    assert!(c_rows.roles.is_empty(), "roles are a signed-statement cache and are not restored");
    let carried = a_rows.oplog.len();
    assert_eq!(&c_rows.oplog[..carried], &a_rows.oplog[..], "op-log identical in count and order");
    let tail: Vec<&str> = c_rows.oplog[carried..].iter().filter_map(|e| e["op_type"].as_str()).collect();
    assert_eq!(tail, ["ImportPetal"], "only the import audit is new");

    // Archive level: the re-export, minus the import audit and with roles carried over, is the same bytes.
    let mut c_comparable = c_rows.clone();
    c_comparable.oplog.truncate(carried);
    c_comparable.roles = a_rows.roles.clone();
    assert_eq!(build_archive(&c_comparable, "2026-11-13T00:00:00Z", None), a_archive, "re-exported archive is byte-identical");
    // Volatile fields (stamps, build id) change without touching the proven content.
    let restamped = archive_format::import_petal_snapshot(&build_archive(&c_comparable, "2027-01-01T00:00:00Z", Some("build-2"))).expect("restamped archive imports");
    assert_eq!(restamped.data, read_back.data, "rows are independent of the volatile stamps");
    assert_eq!(restamped.blobs, read_back.blobs);
    assert_ne!(restamped.manifest.created_at, read_back.manifest.created_at);

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
    seed_fractal(&clock_db).await;
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
    seed_fractal(&refused_db).await;
    let mut foreign_node = snapshot.clone();
    foreign_node.nodes[0]["petal_id"] = json!(SIBLING);
    let mut unreferenced_asset = snapshot.clone();
    unreferenced_asset.assets[0]["asset_id"] = json!("asset-unplaced");
    // Still names PETAL#petal-a, but under another fractal: history may not reach outside the petal's scope.
    let mut foreign_scope = snapshot.clone();
    foreign_scope.oplog.push(json!({
        "id": "op_log:foreign_scope", "lamport_clock": 1, "hlc_timestamp": "1:0001", "node_id": "node-a1",
        "op_type": "TransformUpdate", "payload": {"node_id": "node-a1", "scope": format!("VERSE#{VERSE}-FRACTAL#elsewhere-PETAL#{PETAL}")},
        "sig": "00".repeat(64),
    }));
    // An owner-level audit row is never exported, so a snapshot carrying one was forged.
    let mut forged_audit = snapshot.clone();
    forged_audit.oplog.push(json!({
        "id": "op_log:forged_audit", "lamport_clock": 1, "hlc_timestamp": "1:0001", "node_id": "did:key:owner",
        "op_type": "ElevatedQuery", "payload": {"petal_id": PETAL, "sql": "DELETE role"}, "sig": "00".repeat(64),
    }));
    // Security-relevant and unknown op types are refused by the data allow-list, not by a deny-list.
    let forged_op = |op_type: &str, payload: Value| {
        let mut rows = snapshot.clone();
        rows.oplog.push(json!({
            "id": "op_log:forged_op", "lamport_clock": 1, "hlc_timestamp": "1:0001", "node_id": "did:key:owner",
            "op_type": op_type, "payload": payload, "sig": "00".repeat(64),
        }));
        rows
    };
    let forged_role = forged_op("AssignRole", json!({"peer_did": "did:key:mallory", "scope": petal_scope(PETAL), "role": "owner"}));
    let forged_revoke = forged_op("RevokeSession", json!({"petal_id": PETAL, "session": "s1"}));
    let unknown_type = forged_op("FutureOp", json!({"petal_id": PETAL}));
    let mut malformed_node_id = snapshot.clone();
    malformed_node_id.nodes[0]["node_id"] = json!("node-a1'; DELETE role; --");
    let mut overlong_node_id = snapshot.clone();
    overlong_node_id.nodes[0]["node_id"] = json!("n".repeat(129));
    let mut missing_fractal = snapshot.clone();
    missing_fractal.petal["fractal_id"] = json!("fractal-gone");
    for (case, rows) in [
        ("foreign node", foreign_node),
        ("unreferenced asset", unreferenced_asset),
        ("op-log scope outside the petal", foreign_scope),
        ("forged owner audit", forged_audit),
        ("forged role statement", forged_role),
        ("forged session revocation", forged_revoke),
        ("unknown op type", unknown_type),
        ("malformed node_id", malformed_node_id),
        ("overlong node_id", overlong_node_id),
        ("parent fractal absent", missing_fractal),
    ] {
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

    // Identity leg: a snapshot node whose node_id already lives in another petal here cannot take it over.
    let collision_db = fresh_db().await;
    seed_fractal(&collision_db).await;
    seed_node(&collision_db, "petal-elsewhere", "node-a1", None).await;
    let reply = import_petal_snapshot_reply(&collision_db, snapshot.clone(), Some("collide-1".into())).await;
    assert!(
        matches!(&reply, DbResult::PetalSnapshotImportRefused { reason, correlation_id: Some(id), .. }
            if reason.contains("collide") && id == "collide-1"),
        "a colliding node_id is refused with its reason: {reply:?}"
    );
    assert!(select(&collision_db, "SELECT * FROM petal").await.is_empty(), "nothing restored");
    let owners = select(&collision_db, "SELECT VALUE petal_id FROM node WHERE node_id = 'node-a1'").await;
    assert_eq!(owners, [json!("petal-elsewhere")], "the existing node keeps its petal");
}
