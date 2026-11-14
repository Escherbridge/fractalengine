//! Petal snapshot export: every petal-state row plus the petal's op-log, read in one transaction; see `AGENTS.md` §petal-snapshot.

use std::collections::HashSet;

use anyhow::Context;
use serde_json::Value;
use tracing::instrument;

use fe_runtime::messages::DbResult;
use fe_runtime::petal_snapshot::{PetalSnapshotRows, OPLOG_PREDICATE_VERSION};

use crate::handlers::correlated_failure;
use crate::op_log::{commit_operation, most_specific_error};
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

