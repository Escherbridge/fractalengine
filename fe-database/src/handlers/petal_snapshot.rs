//! Petal snapshot export (one read transaction) and import (one restore transaction); see `AGENTS.md` §petal-snapshot.

use std::collections::HashSet;

use anyhow::Context;
use serde_json::{json, Value};
use tracing::instrument;

use fe_runtime::messages::DbResult;
use fe_runtime::petal_snapshot::{PetalSnapshotRows, OPLOG_PREDICATE_VERSION};

use crate::handlers::correlated_failure;
use crate::op_log::{admit_scene_hlc, advance_hlc_past, commit_operation, most_specific_error};
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// One read transaction: three id LETs, then the ten table SELECTs in [`PetalSnapshotRows`] field order.
/// The op-log SELECT is the coarse SQL half of predicate v1; [`oplog_entry_matches_petal`] is the exact half.
const EXPORT_SCRIPT: &str = "BEGIN TRANSACTION;
LET $petal_node_ids = (SELECT VALUE node_id FROM node WHERE petal_id = $petal);
LET $node_asset_ids = (SELECT VALUE asset_id FROM node WHERE petal_id = $petal);
LET $model_asset_ids = (SELECT VALUE asset_id FROM model WHERE petal_id = $petal);
SELECT * FROM petal WHERE petal_id = $petal LIMIT 2;
SELECT * FROM node WHERE petal_id = $petal ORDER BY node_id ASC;
SELECT * FROM node_log WHERE node_id IN $petal_node_ids ORDER BY node_id ASC, row_version ASC;
SELECT * FROM field_def WHERE scope = $scope OR string::starts_with(scope, $scope_child) ORDER BY field_def_id ASC;
SELECT * FROM room WHERE petal_id = $petal ORDER BY name ASC;
SELECT * FROM model WHERE petal_id = $petal ORDER BY asset_id ASC;
SELECT * FROM asset WHERE asset_id IN $node_asset_ids OR asset_id IN $model_asset_ids ORDER BY asset_id ASC;
SELECT * FROM role WHERE scope = $scope ORDER BY peer_did ASC;
SELECT * FROM iot_reading WHERE petal_id = $petal ORDER BY recorded_at_ms ASC, reading_id ASC;
SELECT * FROM op_log WHERE payload.petal_id = $petal OR payload.scope CONTAINS $petal_segment OR node_id IN $petal_node_ids OR payload.node_id IN $petal_node_ids ORDER BY lamport_clock ASC;
COMMIT TRANSACTION;";

/// Result slot of the first SELECT: `BEGIN` and the three `LET`s come first.
const FIRST_SELECT_SLOT: usize = 4;
/// SELECT statements in [`EXPORT_SCRIPT`].
const SELECT_COUNT: usize = 10;
/// Every result slot of [`EXPORT_SCRIPT`], `COMMIT` included.
const SCRIPT_SLOTS: usize = FIRST_SELECT_SLOT + SELECT_COUNT + 1;

/// Collect a petal's full state verbatim, then record the export as an `ExportPetal` op-log entry.
#[instrument(skip(db))]
pub async fn export_petal_snapshot(db: &Db, petal_id: &str) -> anyhow::Result<PetalSnapshotRows> {
    let scope = crate::handlers::crud::resolve_petal_scope_handler(db, petal_id)
        .await
        .context("petal scope lookup failed")?
        .ok_or_else(|| anyhow::anyhow!("no petal {petal_id} with a resolvable verse/fractal scope"))?;
    let rows = collect_petal_rows(db, petal_id, &scope).await?;

    // After collection, so the snapshot never contains its own export record.
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(petal_id.to_string()),
        op_type: OpType::ExportPetal,
        payload: serde_json::json!({
            "petal_id": petal_id,
            "oplog_predicate_version": OPLOG_PREDICATE_VERSION,
        }),
        sig: "00".repeat(64),
    };
    commit_operation(db, entry, |_lamport| async { Ok::<(), anyhow::Error>(()) })
        .await
        .context("recording the ExportPetal op-log entry failed")?;
    Ok(rows)
}

/// Run [`EXPORT_SCRIPT`] and apply the exact op-log predicate; read-only.
async fn collect_petal_rows(db: &Db, petal_id: &str, scope: &str) -> anyhow::Result<PetalSnapshotRows> {
    let mut response = db
        .query(EXPORT_SCRIPT)
        .bind(("petal", petal_id.to_string()))
        .bind(("scope", scope.to_string()))
        .bind(("scope_child", format!("{scope}-")))
        .bind(("petal_segment", format!("PETAL#{petal_id}")))
        .await
        .context("petal snapshot read failed")?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        anyhow::bail!("petal snapshot read failed: {}", most_specific_error(errors));
    }
    let slots = response.num_statements();
    anyhow::ensure!(
        slots == SCRIPT_SLOTS,
        "petal snapshot read returned {slots} result slots, expected {SCRIPT_SLOTS}"
    );
    let mut tables = Vec::with_capacity(SELECT_COUNT);
    for slot in FIRST_SELECT_SLOT..FIRST_SELECT_SLOT + SELECT_COUNT {
        let table: Vec<Value> = response
            .take(slot)
            .with_context(|| format!("reading petal snapshot slot {slot}"))?;
        tables.push(table);
    }
    let mut tables = tables.into_iter();
    let mut next = || tables.next().unwrap_or_default();

    let mut petal_rows = next();
    anyhow::ensure!(petal_rows.len() == 1, "expected one petal row for {petal_id}, found {}", petal_rows.len());
    let petal = petal_rows.remove(0);
    let nodes = next();
    let node_log = next();
    let field_defs = next();
    let rooms = next();
    let models = next();
    let assets = next();
    let roles = next();
    let iot_readings = next();
    let candidate_oplog = next();

    let petal_node_ids: HashSet<&str> = nodes
        .iter()
        .filter_map(|node| node.get("node_id").and_then(Value::as_str))
        .collect();
    let oplog: Vec<Value> = candidate_oplog
        .into_iter()
        .filter(|entry| oplog_entry_matches_petal(entry, petal_id, &petal_node_ids))
        .filter(|entry| is_importable_op_type(entry["op_type"].as_str().unwrap_or_default()))
        .collect();

    Ok(PetalSnapshotRows {
        petal,
        nodes,
        node_log,
        field_defs,
        rooms,
        models,
        assets,
        roles,
        iot_readings,
        oplog,
    })
}

/// Op-log predicate v1: payload `petal_id` is the petal, OR a payload scope names the petal's segment,
/// OR the entry's own or payload `node_id` is one of the petal's nodes.
fn oplog_entry_matches_petal(entry: &Value, petal_id: &str, petal_node_ids: &HashSet<&str>) -> bool {
    let payload = &entry["payload"];
    let names_petal_node = |value: &Value| value.as_str().is_some_and(|id| petal_node_ids.contains(id));
    payload["petal_id"].as_str() == Some(petal_id)
        || payload["scope"].as_str().is_some_and(|scope| scope_names_petal(scope, petal_id))
        || names_petal_node(&entry["node_id"])
        || names_petal_node(&payload["node_id"])
}

/// Whether `scope` holds a `PETAL#<petal_id>` segment ending at the string end or at a `-KEYWORD#` boundary,
/// so petal `p` never matches `PETAL#p-2` or `PETAL#p2`.
fn scope_names_petal(scope: &str, petal_id: &str) -> bool {
    let segment = format!("PETAL#{petal_id}");
    scope.match_indices(&segment).any(|(start, _)| {
        let rest = &scope[start + segment.len()..];
        if rest.is_empty() {
            return true;
        }
        let Some(next_segment) = rest.strip_prefix('-') else {
            return false;
        };
        next_segment
            .split_once('#')
            .is_some_and(|(keyword, _)| !keyword.is_empty() && keyword.chars().all(|c| c.is_ascii_uppercase()))
    })
}

/// DB-thread reply to `DbCommand::ExportPetalSnapshot`, echoing `correlation_id`.
pub async fn export_petal_snapshot_reply(db: &Db, petal_id: String, correlation_id: Option<String>) -> DbResult {
    match export_petal_snapshot(db, &petal_id).await {
        Ok(rows) => DbResult::PetalSnapshotExported {
            petal_id,
            rows: Box::new(rows),
            correlation_id,
        },
        Err(error) => correlated_failure(&correlation_id, format!("ExportPetalSnapshot {petal_id} failed: {error:#}")),
    }
}

/// Why a petal snapshot import was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// A petal row with the snapshot's `petal_id` already exists; imports never merge.
    #[error("petal already exists")]
    PetalExists,
    /// The rows are malformed or not attributable to the snapshot's petal; nothing was written.
    #[error("invalid petal snapshot: {0}")]
    Invalid(String),
    /// A node, reading or op-log marker in the snapshot already exists here under another owner; nothing was written.
    #[error("snapshot rows collide with existing local rows")]
    Collision,
    /// The restore transaction failed and was cancelled, then cascade-cleaned; no partial petal remains.
    #[error("petal snapshot restore failed: {0:#}")]
    Db(anyhow::Error),
}

/// THROW text of the in-transaction existence guard, mapped back to [`ImportError::PetalExists`].
const PETAL_EXISTS_MARKER: &str = "petal_snapshot_import_petal_exists";
/// THROW text of the in-transaction identity guards, mapped back to [`ImportError::Collision`].
const COLLISION_MARKER: &str = "petal_snapshot_import_collision";
/// Allow-list of data op types a snapshot may carry; role, session, audit and elevated-query types never travel.
const IMPORTABLE_OP_TYPES: [&str; 23] = [
    "SceneAssetAdopted", "EditorTransaction", "PluginTransaction", "CreatePetal", "CreateRoom", "PlaceModel",
    "DeletePetal", "UpdatePetalMeta", "UpdateRoomMeta", "UpdateModelMeta", "ExportPetal", "ImportPetal",
    "TransformUpdate", "PropertySet", "PropertyDeleted", "NodeCreated", "NodeRenamed", "NodeDeleted",
    "NodeTombstoned", "NodePromoted", "SetPetalRenderDistance", "SetPetalTerrain",
    "SetPetalRequirements",
];

/// Whether an op_log row's `op_type` is on the data allow-list.
fn is_importable_op_type(op_type: &str) -> bool {
    IMPORTABLE_OP_TYPES.contains(&op_type)
}

/// Longest accepted node_id; stamp ids are `<path_id>#inst-<n>`.
const MAX_NODE_ID_LEN: usize = 128;

/// Node ids are ULIDs or `<path_id>#inst-<n>` stamp ids: a bounded, printable charset with no quotes or whitespace.
fn is_well_formed_node_id(node_id: &str) -> bool {
    !node_id.is_empty()
        && node_id.len() <= MAX_NODE_ID_LEN
        && node_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'#' | b'.' | b':'))
}
/// Payload fields other writers look op-log rows up by (request and operation ids); imported ones must be fresh.
const OPLOG_MARKER_FIELDS: [&str; 5] = [
    "editor_batch_request_id",
    "editor_request_id",
    "stamp_request_id",
    "operation_id",
    "group_restore_of",
];

/// Defensive cleanup after a failed restore: every row keyed to the petal id, the petal row last.
const CASCADE_SCRIPT: &str = "BEGIN TRANSACTION;
LET $cascade_node_ids = (SELECT VALUE node_id FROM node WHERE petal_id = $petal);
DELETE node_log WHERE node_id IN $cascade_node_ids;
DELETE node WHERE petal_id = $petal;
DELETE room WHERE petal_id = $petal;
DELETE model WHERE petal_id = $petal;
DELETE iot_reading WHERE petal_id = $petal;
DELETE field_def WHERE scope = $petal_scope OR string::starts_with(scope, $petal_scope_child);
DELETE petal WHERE petal_id = $petal;
COMMIT TRANSACTION;";

/// Restore a snapshot verbatim in one transaction, advance the HLC past its clocks, then record `ImportPetal`.
#[instrument(skip_all)]
pub async fn import_petal_snapshot(db: &Db, rows: PetalSnapshotRows) -> Result<(), ImportError> {
    let petal_id = rows.petal_id();
    if petal_id.is_empty() {
        return Err(invalid("the petal row has no petal_id"));
    }
    if petal_exists(db, &petal_id).await.map_err(ImportError::Db)? {
        return Err(ImportError::PetalExists);
    }
    // Import never creates a fractal: the parent must exist here, and it fixes the petal's scope.
    let fractal_id = string_field(&rows.petal, "fractal_id")
        .filter(|fractal_id| !fractal_id.is_empty())
        .ok_or_else(|| invalid("the petal row has no fractal_id"))?
        .to_string();
    let fractal_scope = crate::handlers::crud::resolve_fractal_scope_handler(db, &fractal_id)
        .await
        .map_err(ImportError::Db)?
        .ok_or_else(|| invalid("the snapshot's parent fractal does not exist here"))?;
    let petal_scope = format!("{fractal_scope}-PETAL#{petal_id}");
    let source_node_id = string_field(&rows.petal, "node_id").unwrap_or_default().to_string();
    let plan = RestorePlan::build(&petal_id, &petal_scope, rows)?;
    let clock_maxima = plan.clock_maxima;

    if let Err(error) = run_restore(db, plan).await {
        let text = format!("{error:#}");
        // Guard refusals cancel the transaction before any write: nothing to clean, and nothing ours to delete.
        if text.contains(PETAL_EXISTS_MARKER) {
            return Err(ImportError::PetalExists);
        }
        if text.contains(COLLISION_MARKER) {
            return Err(ImportError::Collision);
        }
        if let Err(cleanup) = cascade_delete_petal(db, &petal_id, &petal_scope).await {
            tracing::warn!(petal_id = %petal_id, "petal snapshot cascade cleanup failed: {cleanup:#}");
        }
        return Err(ImportError::Db(error));
    }

    // Raise-only, per clock family, so post-import writes order after every restored clock.
    for clock in clock_maxima.into_iter().filter(|clock| *clock > 0) {
        advance_hlc_past(clock);
    }
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(petal_id.clone()),
        op_type: OpType::ImportPetal,
        payload: json!({ "petal_id": petal_id, "source_node_id": source_node_id }),
        sig: "00".repeat(64),
    };
    if let Err(error) = commit_operation(db, entry, |_lamport| async { Ok::<(), anyhow::Error>(()) }).await {
        tracing::warn!(petal_id = %petal_id, "petal snapshot restored but its ImportPetal op-log entry failed: {error:#}");
    }
    Ok(())
}

/// DB-thread reply to `DbCommand::ImportPetalSnapshot`, echoing `correlation_id`.
pub async fn import_petal_snapshot_reply(
    db: &Db,
    rows: PetalSnapshotRows,
    correlation_id: Option<String>,
) -> DbResult {
    let petal_id = rows.petal_id();
    match import_petal_snapshot(db, rows).await {
        Ok(()) => DbResult::PetalSnapshotImported {
            petal_id,
            correlation_id,
        },
        // Refusals carry their reason; nothing was written for any of them.
        Err(error @ (ImportError::PetalExists | ImportError::Collision | ImportError::Invalid(_))) => {
            DbResult::PetalSnapshotImportRefused {
                petal_id,
                reason: error.to_string(),
                correlation_id,
            }
        }
        Err(error) => correlated_failure(&correlation_id, format!("ImportPetalSnapshot {petal_id} failed: {error}")),
    }
}

/// The validated restore transaction: script, its bindings and the highest restored clock per family.
struct RestorePlan {
    /// Guards first (they must see only pre-existing rows), then the writes.
    script: String,
    /// Write statements, appended after every guard when the plan is finished.
    writes: String,
    bindings: Vec<(String, Value)>,
    /// Max op-log `lamport_clock`, node-log `hlc_timestamp` and reading `hlc_timestamp`.
    clock_maxima: [u64; 3],
}

impl RestorePlan {
    /// Validate every row against the snapshot petal and its scope, render one `BEGIN … COMMIT` script; writes nothing.
    fn build(petal_id: &str, petal_scope: &str, rows: PetalSnapshotRows) -> Result<Self, ImportError> {
        let PetalSnapshotRows {
            mut petal,
            nodes,
            node_log,
            field_defs,
            rooms,
            models,
            assets,
            roles,
            iot_readings,
            oplog,
        } = rows;
        let mut plan = Self {
            script: String::from("BEGIN TRANSACTION;\n"),
            writes: String::new(),
            bindings: Vec::new(),
            clock_maxima: [0; 3],
        };
        // Any row already keyed to this petal id (the petal, or orphans of it) refuses the import, so the
        // failure cascade below can only ever delete rows this import wrote.
        let petal_binding = plan.bind(json!(petal_id));
        let petal_tables = ["petal", "node", "room", "model", "iot_reading"]
            .map(|table| format!("array::len((SELECT VALUE id FROM {table} WHERE petal_id = ${petal_binding})) > 0"))
            .join(" OR ");
        let scope_child = format!("{petal_scope}-");
        let scope_binding = plan.bind(json!(petal_scope));
        let scope_child_binding = plan.bind(json!(scope_child));
        plan.guard(format!(
            "IF {petal_tables} OR array::len((SELECT VALUE id FROM field_def WHERE scope = ${scope_binding} OR string::starts_with(scope, ${scope_child_binding}))) > 0 {{ THROW '{PETAL_EXISTS_MARKER}' }}"
        ));
        let inside_petal_scope = |scope: &str| scope == petal_scope || scope.starts_with(&scope_child);

        let petal_record = record_id(&petal, "petal")?.to_string();
        let bounds = match take_field(&mut petal, "bounds") {
            None => None,
            Some(bounds) => {
                let rings = polygon_literal(&bounds).ok_or_else(|| invalid("petal bounds are not a finite GeoJSON polygon"))?;
                Some(format!("{{ bounds: <geometry<polygon>> {{ type: 'Polygon', coordinates: {rings} }} }}"))
            }
        };
        let statement = plan.create_statement(&petal_record, petal, bounds);
        plan.push(statement);

        let mut node_ids: HashSet<String> = HashSet::new();
        let mut referenced_assets: HashSet<String> = HashSet::new();
        for mut node in nodes {
            let record = record_id(&node, "node")?.to_string();
            require_petal(&node, "node", petal_id)?;
            let node_id = string_field(&node, "node_id")
                .filter(|node_id| !node_id.is_empty())
                .ok_or_else(|| invalid("a node row has no node_id"))?
                .to_string();
            if !is_well_formed_node_id(&node_id) {
                return Err(invalid("a node row has a malformed node_id"));
            }
            if node_ids.contains(&node_id) {
                return Err(invalid(format!("node_id {node_id} appears twice in the snapshot")));
            }
            if let Some(asset_id) = string_field(&node, "asset_id") {
                referenced_assets.insert(asset_id.to_string());
            }
            let position = take_field(&mut node, "position")
                .as_ref()
                .and_then(point_coordinates)
                .ok_or_else(|| invalid(format!("node {node_id} has no finite GeoJSON point position")))?;
            let x = plan.bind(json!(position.0));
            let z = plan.bind(json!(position.1));
            let statement = plan.create_statement(&record, node, Some(format!("{{ position: <geometry<point>> [${x}, ${z}] }}")));
            plan.push(statement);
            node_ids.insert(node_id);
        }

        for entry in node_log {
            let record = record_id(&entry, "node_log")?.to_string();
            if !string_field(&entry, "node_id").is_some_and(|node_id| node_ids.contains(node_id)) {
                return Err(invalid("a node_log row names a node outside the snapshot"));
            }
            observe_clock(&mut plan.clock_maxima[1], entry.get("hlc_timestamp"), "node_log hlc_timestamp")?;
            let statement = plan.create_statement(&record, entry, None);
            plan.push(statement);
        }

        for field_def in field_defs {
            let record = record_id(&field_def, "field_def")?.to_string();
            if !string_field(&field_def, "scope").is_some_and(inside_petal_scope) {
                return Err(invalid("a field_def row is scoped outside the snapshot petal"));
            }
            let statement = plan.create_statement(&record, field_def, None);
            plan.push(statement);
        }

        for room in rooms {
            let record = record_id(&room, "room")?.to_string();
            require_petal(&room, "room", petal_id)?;
            let statement = plan.create_statement(&record, room, None);
            plan.push(statement);
        }

        // Models and assets: an existing row with the same id is kept (upsert-or-skip).
        for model in models {
            let record = record_id(&model, "model")?.to_string();
            require_petal(&model, "model", petal_id)?;
            if let Some(asset_id) = string_field(&model, "asset_id") {
                referenced_assets.insert(asset_id.to_string());
            }
            let record_binding = plan.bind(json!(record));
            let statement = plan.create_statement(&record, model, None);
            plan.push(format!("IF !record::exists(type::record(${record_binding})) {{ {statement} }}"));
        }

        for asset in assets {
            let record = record_id(&asset, "asset")?.to_string();
            let asset_id = string_field(&asset, "asset_id").ok_or_else(|| invalid("an asset row has no asset_id"))?.to_string();
            if !referenced_assets.contains(&asset_id) {
                return Err(invalid(format!("asset {asset_id} is not referenced by the snapshot's nodes or models")));
            }
            let asset_binding = plan.bind(json!(asset_id));
            if let Some(content_hash) = string_field(&asset, "content_hash") {
                let hash_binding = plan.bind(json!(content_hash));
                plan.push(format!(
                    "IF array::len((SELECT VALUE id FROM asset WHERE asset_id = ${asset_binding} AND content_hash != NONE AND content_hash != ${hash_binding})) > 0 {{ THROW 'petal snapshot asset differs from the local asset with the same asset_id' }}"
                ));
            }
            let record_binding = plan.bind(json!(record));
            let statement = plan.create_statement(&record, asset, None);
            plan.push(format!(
                "IF array::len((SELECT VALUE id FROM asset WHERE asset_id = ${asset_binding} OR id = type::record(${record_binding}))) = 0 {{ {statement} }}"
            ));
        }

        // Role rows are a materialized cache of signed statements (src/AGENTS.md §role-propagation): never restored.
        if !roles.is_empty() {
            tracing::info!(petal_id, skipped = roles.len(), "petal snapshot role rows are not restored");
        }

        // Node ids referenced but not restored (readings or history of deleted nodes): must not exist here.
        let mut dangling_node_ids: HashSet<String> = HashSet::new();
        let mut reading_ids: HashSet<String> = HashSet::new();
        for reading in iot_readings {
            let record = record_id(&reading, "iot_reading")?.to_string();
            require_petal(&reading, "iot_reading", petal_id)?;
            let reading_id = string_field(&reading, "reading_id")
                .filter(|reading_id| !reading_id.is_empty())
                .ok_or_else(|| invalid("an iot_reading row has no reading_id"))?;
            if !reading_ids.insert(reading_id.to_string()) {
                return Err(invalid(format!("reading_id {reading_id} appears twice in the snapshot")));
            }
            if let Some(node_id) = string_field(&reading, "node_id").filter(|node_id| !node_ids.contains(*node_id)) {
                dangling_node_ids.insert(node_id.to_string());
            }
            observe_clock(&mut plan.clock_maxima[2], reading.get("hlc_timestamp"), "iot_reading hlc_timestamp")?;
            let statement = plan.create_statement(&record, reading, None);
            plan.push(statement);
        }

        let node_id_refs: HashSet<&str> = node_ids.iter().map(String::as_str).collect();
        let mut oplog_markers: HashSet<String> = HashSet::new();
        let mut adopted_asset_ids: HashSet<String> = HashSet::new();
        for entry in oplog {
            let record = record_id(&entry, "op_log")?.to_string();
            if !oplog_entry_matches_petal(&entry, petal_id, &node_id_refs) {
                return Err(invalid("an op_log entry is not attributable to the snapshot petal"));
            }
            let op_type = string_field(&entry, "op_type").unwrap_or_default();
            if !is_importable_op_type(op_type) {
                return Err(invalid(format!("an op_log entry of type {op_type:?} is not a data operation a petal snapshot may carry")));
            }
            // Containment: every petal, scope and node reference the payload carries stays inside this petal.
            let payload = &entry["payload"];
            if !matches!(&payload["petal_id"], Value::Null) && payload["petal_id"].as_str() != Some(petal_id) {
                return Err(invalid("an op_log entry names another petal"));
            }
            if !matches!(&payload["scope"], Value::Null) && !payload["scope"].as_str().is_some_and(inside_petal_scope) {
                return Err(invalid("an op_log entry is scoped outside the snapshot petal"));
            }
            match &payload["node_id"] {
                Value::Null => {}
                Value::String(node_id) => {
                    if !node_id_refs.contains(node_id.as_str()) {
                        dangling_node_ids.insert(node_id.clone());
                    }
                }
                _ => return Err(invalid("an op_log entry has a non-string payload node_id")),
            }
            for field in OPLOG_MARKER_FIELDS {
                if let Some(marker) = payload[field].as_str() {
                    oplog_markers.insert(marker.to_string());
                }
            }
            if op_type == "SceneAssetAdopted" {
                if let Some(asset_id) = payload["asset_id"].as_str() {
                    adopted_asset_ids.insert(asset_id.to_string());
                }
            }
            let lamport = entry
                .get("lamport_clock")
                .filter(|clock| clock.as_u64().is_some_and(|clock| clock > 0));
            if lamport.is_none() {
                return Err(invalid("an op_log entry has no positive lamport_clock"));
            }
            observe_clock(&mut plan.clock_maxima[0], lamport, "op_log lamport_clock")?;
            let statement = plan.create_statement(&record, entry, None);
            plan.push(statement);
        }

        // Identity guards (security inbox 5, 6, 8): every imported identity other writers look up must be
        // new here, so a snapshot can never take over or shadow another scope's rows.
        let mut node_identities = node_ids;
        node_identities.extend(dangling_node_ids);
        plan.collision_guard(|set| format!("SELECT VALUE id FROM node WHERE node_id IN ${set}"), node_identities);
        plan.collision_guard(|set| format!("SELECT VALUE id FROM iot_reading WHERE reading_id IN ${set}"), reading_ids);
        let marker_lookup = OPLOG_MARKER_FIELDS
            .map(|field| format!("payload.{field} IN $"))
            .to_vec();
        plan.collision_guard(
            |set| {
                let clauses: Vec<String> = marker_lookup.iter().map(|clause| format!("{clause}{set}")).collect();
                format!("SELECT VALUE id FROM op_log WHERE {}", clauses.join(" OR "))
            },
            oplog_markers,
        );
        plan.collision_guard(
            |set| format!("SELECT VALUE id FROM op_log WHERE op_type = 'SceneAssetAdopted' AND payload.asset_id IN ${set}"),
            adopted_asset_ids,
        );

        let writes = std::mem::take(&mut plan.writes);
        plan.script.push_str(&writes);
        plan.script.push_str("COMMIT TRANSACTION;");
        Ok(plan)
    }

    /// Bind `value` under a fresh `import_<n>` name and return that name.
    fn bind(&mut self, value: Value) -> String {
        let name = format!("import_{}", self.bindings.len());
        self.bindings.push((name.clone(), value));
        name
    }

    /// `CREATE` at the row's original record id, content minus `id`, plus an optional cast `extension` object.
    fn create_statement(&mut self, record: &str, mut row: Value, extension: Option<String>) -> String {
        if let Some(object) = row.as_object_mut() {
            object.remove("id");
        }
        let record_binding = self.bind(json!(record));
        let row_binding = self.bind(crate::repo::strip_json_nulls(row));
        let content = match extension {
            Some(extension) => format!("object::extend(${row_binding}, {extension})"),
            None => format!("${row_binding}"),
        };
        format!("CREATE type::record(${record_binding}) CONTENT {content}")
    }

    /// Append one write statement.
    fn push(&mut self, statement: String) {
        self.writes.push_str(&statement);
        self.writes.push_str(";\n");
    }

    /// Append one guard; guards run before every write.
    fn guard(&mut self, statement: String) {
        self.script.push_str(&statement);
        self.script.push_str(";\n");
    }

    /// Guard: THROW [`COLLISION_MARKER`] when `select` (a `SELECT VALUE id … IN $set`) finds any row for `values`.
    fn collision_guard(&mut self, select: impl FnOnce(&str) -> String, values: HashSet<String>) {
        if values.is_empty() {
            return;
        }
        let mut values: Vec<String> = values.into_iter().collect();
        values.sort();
        let binding = self.bind(json!(values));
        let select = select(&binding);
        self.guard(format!("IF array::len(({select})) > 0 {{ THROW '{COLLISION_MARKER}' }}"));
    }
}

/// Run the restore script; any statement error cancels the whole transaction.
async fn run_restore(db: &Db, plan: RestorePlan) -> anyhow::Result<()> {
    let mut query = db.query(plan.script);
    for binding in plan.bindings {
        query = query.bind(binding);
    }
    let mut response = query.await.context("petal snapshot restore failed")?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        anyhow::bail!("{}", most_specific_error(errors));
    }
    Ok(())
}

/// Whether a petal row with `petal_id` exists.
async fn petal_exists(db: &Db, petal_id: &str) -> anyhow::Result<bool> {
    let mut response = db
        .query("SELECT VALUE id FROM petal WHERE petal_id = $petal LIMIT 1")
        .bind(("petal", petal_id.to_string()))
        .await
        .context("petal existence check failed")?
        .check()
        .context("petal existence check failed")?;
    let ids: Vec<Value> = response.take(0).context("reading the petal existence check")?;
    Ok(!ids.is_empty())
}

/// Delete everything keyed to `petal_id` ([`CASCADE_SCRIPT`]); only called after a failed restore.
async fn cascade_delete_petal(db: &Db, petal_id: &str, petal_scope: &str) -> anyhow::Result<()> {
    db.query(CASCADE_SCRIPT)
        .bind(("petal", petal_id.to_string()))
        .bind(("petal_scope", petal_scope.to_string()))
        .bind(("petal_scope_child", format!("{petal_scope}-")))
        .await
        .context("petal snapshot cascade failed")?
        .check()
        .context("petal snapshot cascade failed")?;
    Ok(())
}

fn invalid(reason: impl Into<String>) -> ImportError {
    ImportError::Invalid(reason.into())
}

/// The row's `<table>:<key>` record id, refused when absent or naming another table.
fn record_id<'a>(row: &'a Value, table: &str) -> Result<&'a str, ImportError> {
    let id = string_field(row, "id").unwrap_or_default();
    let key = id
        .strip_prefix(table)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or_default();
    if key.is_empty() {
        return Err(invalid(format!("a {table} row has no `{table}:` record id")));
    }
    Ok(id)
}

fn string_field<'a>(row: &'a Value, field: &str) -> Option<&'a str> {
    row.get(field).and_then(Value::as_str)
}

fn require_petal(row: &Value, table: &str, petal_id: &str) -> Result<(), ImportError> {
    if string_field(row, "petal_id") == Some(petal_id) {
        Ok(())
    } else {
        Err(invalid(format!("a {table} row belongs to another petal")))
    }
}

/// Remove `field` from an object row, returning it unless it was null.
fn take_field(row: &mut Value, field: &str) -> Option<Value> {
    row.as_object_mut()?.remove(field).filter(|value| !value.is_null())
}

/// `(x, z)` of a finite GeoJSON Point.
fn point_coordinates(point: &Value) -> Option<(f64, f64)> {
    if point.get("type")?.as_str()? != "Point" {
        return None;
    }
    let coordinates = point.get("coordinates")?.as_array()?;
    let (x, z) = (coordinates.first()?.as_f64()?, coordinates.get(1)?.as_f64()?);
    (coordinates.len() == 2 && x.is_finite() && z.is_finite()).then_some((x, z))
}

/// Inline numeric rings literal of a finite GeoJSON Polygon (the only form a polygon cast accepts, src/AGENTS.md §geometry-inserts).
fn polygon_literal(polygon: &Value) -> Option<String> {
    if polygon.get("type")?.as_str()? != "Polygon" {
        return None;
    }
    let mut rings = Vec::new();
    for ring in polygon.get("coordinates")?.as_array()? {
        let mut points = Vec::new();
        for point in ring.as_array()? {
            let pair = point.as_array()?;
            let (x, z) = (pair.first()?.as_f64()?, pair.get(1)?.as_f64()?);
            if pair.len() != 2 || !x.is_finite() || !z.is_finite() {
                return None;
            }
            points.push(format!("[{x:?}, {z:?}]"));
        }
        rings.push(format!("[{}]", points.join(", ")));
    }
    Some(format!("[{}]", rings.join(", ")))
}

/// Fold one restored clock into `maximum`; a non-integer or out-of-bound clock refuses the import.
fn observe_clock(maximum: &mut u64, clock: Option<&Value>, what: &str) -> Result<(), ImportError> {
    let Some(clock) = clock.filter(|clock| !clock.is_null()) else {
        return Ok(());
    };
    let clock = clock
        .as_u64()
        .ok_or_else(|| invalid(format!("a {what} is not an unsigned integer clock")))?;
    if clock == 0 {
        return Ok(());
    }
    admit_scene_hlc(clock).map_err(|error| invalid(format!("a {what} is outside the admitted clock range: {error}")))?;
    *maximum = (*maximum).max(clock);
    Ok(())
}

