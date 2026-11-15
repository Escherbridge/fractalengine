//! Petal snapshot export (one read transaction) and import (one restore transaction); see `AGENTS.md` §petal-snapshot.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use serde_json::{json, Value};
use tracing::instrument;

use fe_runtime::messages::DbResult;
use fe_runtime::petal_snapshot::{PetalSnapshotRows, OPLOG_PREDICATE_VERSION};

use crate::handlers::correlated_failure;
use crate::handlers::node_history::ARCHIVE_FACT_KEY;
use crate::handlers::object_creation::is_admissible_transform;
use crate::handlers::scene_publication::budget::{node_write_budget_for_verse, ByteRate, NodeWriteDoor};
use crate::op_log::{admit_scene_hlc, advance_hlc_past, commit_operation, most_specific_error};
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// One read transaction: three id LETs, the ten table SELECTs in [`PetalSnapshotRows`] field order, then quarantined history.
/// The op-log SELECT is the coarse SQL half of predicate v1; [`oplog_entry_matches_petal`] is the exact half.
/// The journal slot reads only each node's newest version (or nothing); its facts are read after, bounded (§export-bound).
const EXPORT_SCRIPT: &str = "BEGIN TRANSACTION;
LET $petal_node_ids = (SELECT VALUE node_id FROM node WHERE petal_id = $petal);
LET $node_asset_ids = (SELECT VALUE asset_id FROM node WHERE petal_id = $petal);
LET $model_asset_ids = (SELECT VALUE asset_id FROM model WHERE petal_id = $petal);
SELECT * FROM petal WHERE petal_id = $petal LIMIT 2;
SELECT * FROM node WHERE petal_id = $petal ORDER BY node_id ASC;
JOURNAL_SLOT;
SELECT * FROM field_def WHERE scope = $scope OR string::starts_with(scope, $scope_child) ORDER BY field_def_id ASC;
SELECT * FROM room WHERE petal_id = $petal ORDER BY name ASC;
SELECT * FROM model WHERE petal_id = $petal ORDER BY asset_id ASC;
SELECT * FROM asset WHERE asset_id IN $node_asset_ids OR asset_id IN $model_asset_ids ORDER BY asset_id ASC;
SELECT * FROM role WHERE scope = $scope ORDER BY peer_did ASC;
SELECT * FROM iot_reading WHERE petal_id = $petal ORDER BY recorded_at_ms ASC, reading_id ASC;
SELECT * FROM op_log WHERE payload.petal_id = $petal OR payload.scope CONTAINS $petal_segment OR node_id IN $petal_node_ids OR payload.node_id IN $petal_node_ids ORDER BY lamport_clock ASC;
SELECT * FROM op_log_imported WHERE import_petal_id = $petal ORDER BY lamport_clock ASC;
COMMIT TRANSACTION;";

/// Result slot of the first SELECT: `BEGIN` and the three `LET`s come first.
const FIRST_SELECT_SLOT: usize = 4;
/// SELECT statements in [`EXPORT_SCRIPT`].
const SELECT_COUNT: usize = 11;
/// Every result slot of [`EXPORT_SCRIPT`], `COMMIT` included.
const SCRIPT_SLOTS: usize = FIRST_SELECT_SLOT + SELECT_COUNT + 1;
/// Journal slot when history is exported: each node's newest version at the snapshot instant.
const JOURNAL_CEILINGS: &str =
    "SELECT node_id, math::max(row_version) AS latest FROM node_log WHERE node_id IN $petal_node_ids GROUP BY node_id";
/// Journal slot when no history is exported: a record lookup that reads nothing.
const JOURNAL_NONE: &str = "SELECT node_id FROM node_log:fe_snapshot_export_without_history";

/// Journal facts an export keeps per node by default (newest first); see AGENTS.md §export-bound.
pub const EXPORT_HISTORY_PER_NODE: usize = 8;
/// Serialized journal bytes one export may carry by default, well under the archive member limit.
pub const EXPORT_HISTORY_BYTES: usize = 32 * 1024 * 1024;
/// Prefix of the export refusal for an archive the import side would refuse.
pub const EXPORT_NOT_IMPORTABLE: &str = "petal snapshot would not import";

/// How much journal history an export carries, and the import limits it must stay within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotExportOptions {
    /// Newest journal facts kept per node (0 = none; an import writes its own baseline fact per node).
    pub history_per_node: usize,
    /// Serialized bytes of journal facts across the whole export; nodes in id order, newest facts first.
    pub history_bytes: usize,
    /// Largest serialized state table / op-log member the import reads (`fe_format::limits`).
    pub member_limit_bytes: u64,
    /// Largest total of members plus asset blobs the import reads (`fe_format::limits`).
    pub total_limit_bytes: u64,
}

impl Default for SnapshotExportOptions {
    fn default() -> Self {
        Self {
            history_per_node: EXPORT_HISTORY_PER_NODE,
            history_bytes: EXPORT_HISTORY_BYTES,
            member_limit_bytes: fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
            total_limit_bytes: fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
        }
    }
}

/// Collect a petal's state verbatim with the default bounded history, then record the export as an `ExportPetal` op-log entry.
#[instrument(skip(db))]
pub async fn export_petal_snapshot(db: &Db, petal_id: &str) -> anyhow::Result<PetalSnapshotRows> {
    export_petal_snapshot_with(db, petal_id, SnapshotExportOptions::default()).await
}

/// [`export_petal_snapshot`] with explicit history depth and import limits; refuses (writing nothing) an archive
/// the import would refuse.
#[instrument(skip(db))]
pub async fn export_petal_snapshot_with(
    db: &Db,
    petal_id: &str,
    options: SnapshotExportOptions,
) -> anyhow::Result<PetalSnapshotRows> {
    let scope = crate::handlers::crud::resolve_petal_scope_handler(db, petal_id)
        .await
        .context("petal scope lookup failed")?
        .ok_or_else(|| anyhow::anyhow!("no petal {petal_id} with a resolvable verse/fractal scope"))?;
    let mut rows = collect_petal_rows(db, petal_id, &scope, options.history_per_node > 0).await?;
    let ceilings = std::mem::take(&mut rows.node_log);
    rows.node_log = collect_bounded_history(db, &ceilings, options).await?;
    let estimated = importable_size(&rows, options).map_err(|reason| anyhow::anyhow!("{EXPORT_NOT_IMPORTABLE}: {reason}"))?;
    tracing::debug!(petal_id, estimated, "petal snapshot export sized");

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

/// Run [`EXPORT_SCRIPT`] and apply the exact op-log predicate; read-only. `node_log` holds the journal
/// ceilings (`{node_id, latest}`) when `with_history`, else nothing.
async fn collect_petal_rows(db: &Db, petal_id: &str, scope: &str, with_history: bool) -> anyhow::Result<PetalSnapshotRows> {
    let script = EXPORT_SCRIPT.replace("JOURNAL_SLOT", if with_history { JOURNAL_CEILINGS } else { JOURNAL_NONE });
    let mut response = db
        .query(script)
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
    let quarantined_oplog = next();

    let petal_node_ids: HashSet<&str> = nodes
        .iter()
        .filter_map(|node| node.get("node_id").and_then(Value::as_str))
        .collect();
    // Imported history re-exports in its original shape, so export → import → export is a fixed point.
    // Live rows come first so a row present in both op_log and op_log_imported keeps its live copy (dedup by record id).
    let mut seen_record_ids: HashSet<String> = HashSet::new();
    let mut oplog: Vec<Value> = candidate_oplog
        .into_iter()
        .filter(|entry| oplog_entry_matches_petal(entry, petal_id, &petal_node_ids))
        .chain(quarantined_oplog.into_iter().filter_map(exported_quarantined_row))
        .filter(|entry| is_importable_op_type(entry["op_type"].as_str().unwrap_or_default()))
        .filter(|entry| entry["id"].as_str().is_none_or(|id| seen_record_ids.insert(id.to_string())))
        .collect();
    oplog.sort_by(|left, right| {
        let key = |entry: &Value| (entry["lamport_clock"].as_u64().unwrap_or_default(), entry["id"].as_str().unwrap_or_default().to_string());
        key(left).cmp(&key(right))
    });

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

/// Read each node's newest `history_per_node` journal facts at or below its snapshot ceiling, nodes in id order,
/// until `history_bytes` is spent; returned in `node_id, row_version` order like the full export was.
async fn collect_bounded_history(
    db: &Db,
    ceilings: &[Value],
    options: SnapshotExportOptions,
) -> anyhow::Result<Vec<Value>> {
    if options.history_per_node == 0 {
        return Ok(Vec::new());
    }
    let mut ceilings: Vec<(String, i64)> = ceilings
        .iter()
        .filter_map(|row| Some((row.get("node_id")?.as_str()?.to_string(), row.get("latest")?.as_i64()?)))
        .collect();
    ceilings.sort_unstable();
    let depth = i64::try_from(options.history_per_node).unwrap_or(i64::MAX);
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    'nodes: for (node_id, latest) in ceilings {
        let mut response = db
            .query("SELECT * FROM node_log WHERE node_id = $node AND row_version <= $latest ORDER BY row_version DESC LIMIT $depth")
            .bind(("node", node_id))
            .bind(("latest", latest))
            .bind(("depth", depth))
            .await
            .context("petal snapshot history read failed")?
            .check()
            .context("petal snapshot history read failed")?;
        let facts: Vec<Value> = response.take(0)?;
        for mut fact in facts {
            // The archive marker is local provenance; the next importer stamps its own (export → import fixed point).
            if let Some(payload) = fact.get_mut("payload").and_then(Value::as_object_mut) {
                payload.remove(ARCHIVE_FACT_KEY);
            }
            let size = serde_json::to_vec(&fact)?.len() + 1;
            if bytes + size > options.history_bytes {
                break 'nodes;
            }
            bytes += size;
            kept.push(fact);
        }
    }
    kept.sort_by(|left, right| {
        let key = |fact: &Value| (fact["node_id"].as_str().unwrap_or_default().to_string(), fact["row_version"].as_i64().unwrap_or_default());
        key(left).cmp(&key(right))
    });
    Ok(kept)
}

/// Estimated archive bytes (members plus asset blobs), or why the import side would refuse the archive.
pub fn importable_size(rows: &PetalSnapshotRows, options: SnapshotExportOptions) -> Result<u64, String> {
    fn json_len(rows: &[Value]) -> u64 {
        serde_json::to_vec(rows).map(|bytes| bytes.len() as u64).unwrap_or(u64::MAX)
    }
    let oplog_len: u64 = rows
        .oplog
        .iter()
        .map(|row| serde_json::to_vec(row).map(|bytes| bytes.len() as u64 + 1).unwrap_or(u64::MAX))
        .fold(0u64, u64::saturating_add);
    let members = [
        ("petal", serde_json::to_vec(&rows.petal).map(|bytes| bytes.len() as u64).unwrap_or(u64::MAX)),
        ("nodes", json_len(&rows.nodes)),
        ("node_log", json_len(&rows.node_log)),
        ("field_defs", json_len(&rows.field_defs)),
        ("rooms", json_len(&rows.rooms)),
        ("models", json_len(&rows.models)),
        ("assets", json_len(&rows.assets)),
        ("roles", json_len(&rows.roles)),
        ("iot_readings", json_len(&rows.iot_readings)),
        ("oplog", oplog_len),
    ];
    for (member, size) in members {
        if size > options.member_limit_bytes {
            return Err(format!(
                "its {member} member is {size} bytes, over the {} byte member limit; export less history or split the petal",
                options.member_limit_bytes
            ));
        }
    }
    let blobs: u64 = rows
        .assets
        .iter()
        .filter_map(|asset| asset.get("size_bytes").and_then(Value::as_u64))
        .fold(0u64, u64::saturating_add);
    let total = members.iter().map(|(_, size)| *size).fold(blobs, u64::saturating_add);
    if total > options.total_limit_bytes {
        return Err(format!("it would hold {total} bytes, over the {} byte archive limit", options.total_limit_bytes));
    }
    Ok(total)
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

/// Columns `op_log_imported` adds to an op-log row; dropped again on export.
const QUARANTINE_COLUMNS: [&str; 5] = ["source_id", "import_petal_id", "import_id", "correlation_id", "imported_by"];

/// A quarantined row in its exported `op_log` shape: the original record id back, provenance dropped.
fn exported_quarantined_row(mut row: Value) -> Option<Value> {
    let object = row.as_object_mut()?;
    let source_id = object.get("source_id").and_then(Value::as_str)?.to_string();
    for column in QUARANTINE_COLUMNS {
        object.remove(column);
    }
    object.insert("id".into(), json!(source_id));
    Some(row)
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
    /// A node or reading in the snapshot already exists here under another owner; nothing was written.
    #[error("snapshot rows collide with existing local rows")]
    Collision,
    /// The restore transaction failed and was cancelled, then cascade-cleaned; no partial petal remains.
    #[error("petal snapshot restore failed: {0:#}")]
    Db(anyhow::Error),
    /// A replicating import named no valid importer identity; nothing was written.
    #[error("a replicating petal snapshot import needs the importer's identity")]
    Unattributed,
    /// The importer's node write budget in the destination verse is spent (retryable); nothing was written.
    #[error("{0:#}")]
    WriteBudget(anyhow::Error),
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

/// `coverage_reason` of the journal fact an import writes per node (its publication proof).
pub const IMPORT_COVERAGE_REASON: &str = "petal_imported";
/// Journal version of the import facts (`node_history::JOURNAL_VERSION`).
const JOURNAL_VERSION: u64 = crate::handlers::node_history::JOURNAL_VERSION;

/// Longest accepted node_id; stamp ids are `<path_id>#inst-<n>`.
const MAX_NODE_ID_LEN: usize = 128;

/// Node ids are ULIDs or `<path_id>#inst-<n>` stamp ids: a bounded, printable charset with no quotes or whitespace.
fn is_well_formed_node_id(node_id: &str) -> bool {
    !node_id.is_empty()
        && node_id.len() <= MAX_NODE_ID_LEN
        && node_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'#' | b'.' | b':'))
}

/// Defensive cleanup after a failed restore: every row keyed to the petal id, the petal row last.
const CASCADE_SCRIPT: &str = "BEGIN TRANSACTION;
LET $cascade_node_ids = (SELECT VALUE node_id FROM node WHERE petal_id = $petal);
DELETE node_log WHERE node_id IN $cascade_node_ids;
DELETE node WHERE petal_id = $petal;
DELETE room WHERE petal_id = $petal;
DELETE model WHERE petal_id = $petal;
DELETE iot_reading WHERE petal_id = $petal;
DELETE op_log_imported WHERE import_petal_id = $petal;
DELETE field_def WHERE scope = $petal_scope OR string::starts_with(scope, $petal_scope_child);
DELETE petal WHERE petal_id = $petal;
COMMIT TRANSACTION;";

/// What a committed import wrote that replication needs (see `AGENTS.md` §petal-snapshot-replication).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedPetal {
    pub petal_id: String,
    /// Every imported node, in snapshot order; each carries a fresh import journal fact when an actor imported it.
    pub node_ids: Vec<String>,
    /// Snapshot role rows that were not imported (role grants never come out of a file).
    pub roles_not_imported: usize,
    /// Destination verse, the importer charged for the import (§write-budget) and this import's id.
    pub(crate) verse_id: String,
    pub(crate) actor: Option<String>,
    pub(crate) import_id: String,
}

/// The importer's local node write budget (handlers/AGENTS.md §write-budget); the DB thread lends its own door.
pub struct ImportWriteBudget {
    door: NodeWriteDoor,
}

impl ImportWriteBudget {
    /// The production local budget on a clock that never advances, so the balance shows exactly what was charged.
    pub fn frozen() -> Self {
        let now = std::time::Instant::now();
        Self { door: NodeWriteDoor::with(node_write_budget_for_verse, Box::new(move || now)) }
    }

    /// A frozen budget whose burst is one byte: it admits one write per author, which then leaves that author in debt.
    pub fn frozen_one_write() -> Self {
        fn one_byte(_verse_id: &str) -> ByteRate {
            ByteRate { burst_bytes: 1, ..node_write_budget_for_verse("") }
        }
        let now = std::time::Instant::now();
        Self { door: NodeWriteDoor::with(one_byte, Box::new(move || now)) }
    }

    /// `actor`'s balance in bytes in `verse_id`; negative while in debt.
    pub fn balance(&mut self, verse_id: &str, actor: &str) -> i64 {
        self.door.balance(verse_id, actor)
    }
}

/// How far post-commit publication got; never a delivery certificate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportPublication {
    pub petal_enqueued: bool,
    /// Nodes handed to the post-commit publisher (it logs and defers any it cannot prove or enqueue).
    pub nodes_attempted: usize,
    /// Nodes left for replica-open recovery without an attempt (no sender, or the queue was full).
    pub nodes_deferred: usize,
}

/// Restore a snapshot verbatim in one transaction, advance the HLC past its clocks, then record `ImportPetal`.
pub async fn import_petal_snapshot(db: &Db, rows: PetalSnapshotRows) -> Result<(), ImportError> {
    import_petal_snapshot_correlated(db, None, rows, None, None, None).await.map(drop)
}

/// [`import_petal_snapshot`] stamping `actor` (the importer's DID) on the quarantined history and its journal facts.
pub async fn import_petal_snapshot_as(
    db: &Db,
    rows: PetalSnapshotRows,
    actor: Option<&str>,
) -> Result<(), ImportError> {
    import_petal_snapshot_correlated(db, None, rows, None, actor, None).await.map(drop)
}

/// Import as `actor` at `budget`, clocking the petal row as `publisher` (this host's DID), so the result is publishable.
pub async fn import_petal_snapshot_publishable(
    db: &Db,
    budget: &mut ImportWriteBudget,
    rows: PetalSnapshotRows,
    actor: &str,
    publisher: &str,
) -> Result<ImportedPetal, ImportError> {
    import_petal_snapshot_correlated(db, Some(&mut budget.door), rows, None, Some(actor), Some(publisher)).await
}

/// [`import_petal_snapshot`] with the caller's correlation id and actor stamped on the quarantined history.
/// A replicating import (`publisher`) needs a valid actor and passes the actor's write budget at `door`.
#[instrument(skip_all)]
async fn import_petal_snapshot_correlated(
    db: &Db,
    door: Option<&mut NodeWriteDoor>,
    rows: PetalSnapshotRows,
    correlation_id: Option<&str>,
    actor: Option<&str>,
    publisher: Option<&str>,
) -> Result<ImportedPetal, ImportError> {
    let actor = actor.filter(|actor| !actor.is_empty() && actor.len() <= MAXIMUM_ACTOR_LENGTH);
    // No identity, no facts: an unattributed replicating import would leave archive history as the newest fact.
    if publisher.is_some() && actor.is_none() {
        return Err(ImportError::Unattributed);
    }
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
    let verse_id = crate::parse_scope(&fractal_scope)
        .map_err(|error| invalid(format!("the parent fractal's scope is malformed: {error}")))?
        .verse_id;
    // Admitted while the importer's balance is positive; charged after commit, so a large import may end in debt.
    if let (Some(door), Some(actor)) = (door, actor) {
        door.admit_scope(&fractal_scope, actor).map_err(ImportError::WriteBudget)?;
    }
    let source_node_id = string_field(&rows.petal, "node_id").unwrap_or_default().to_string();
    let import_id = ulid::Ulid::new().to_string();
    let provenance = Provenance { import_id: &import_id, correlation_id, actor };
    let roles_not_imported = rows.roles.len();
    let mut plan = RestorePlan::build(&petal_id, &petal_scope, &provenance, rows)?;
    let node_ids = plan.node_ids.clone();
    // The live property-write rule: every imported value passes the enforced schemas in effect at the destination
    // (verse, fractal) and the archive's own petal rules, before anything is written (§petal-snapshot-replication).
    let mut rules_by_key: HashMap<String, Vec<Value>> = HashMap::new();
    for (node_id, snapshot) in &plan.projected_nodes {
        let Some(properties) = snapshot.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in properties {
            if !rules_by_key.contains_key(key) {
                let mut rules = super::field_def::capture_property_rules(db, &petal_scope, key)
                    .await
                    .map_err(ImportError::Db)?;
                rules.extend(plan.property_rules.iter().filter(|rule| rule["key"].as_str() == Some(key.as_str())).cloned());
                rules_by_key.insert(key.clone(), rules);
            }
            super::field_def::validate_captured_property_rules(&petal_scope, key, value, &rules_by_key[key])
                .map_err(|error| invalid(format!("node {node_id} property {key:?}: {error:#}")))?;
        }
    }
    // A node whose published row could never be sent is refused, naming it (scene_publication/AGENTS.md §row-bound).
    let checking_as = publisher.or(actor).unwrap_or(IMPORT_CHECK_PUBLISHER);
    for (node_id, snapshot) in std::mem::take(&mut plan.projected_nodes) {
        super::scene_publication::check_publishable(db, &node_id, checking_as, None, &snapshot)
            .await
            .map_err(|error| invalid(format!("node {node_id}: {error:#}")))?;
    }

    // Raise-only, per clock family, BEFORE the fresh import clock is taken: the import's own facts, stamps and
    // audit entry order after every restored clock, and so does every later write.
    for clock in plan.clock_maxima.into_iter().filter(|clock| *clock > 0) {
        advance_hlc_past(clock);
    }
    let import_hlc = crate::op_log::next_hlc_timestamp().0;
    let import_hlc = i64::try_from(import_hlc).map_err(|_| invalid("the local import clock is out of range"))?;
    plan.bind_named("import_fact_hlc", json!(import_hlc));
    plan.bind_named("import_fact_actor", json!(actor.unwrap_or_default()));
    plan.bind_named("import_fact_scope", json!(petal_scope));
    plan.bind_named("import_fact_at", json!(chrono::Utc::now().to_rfc3339()));
    if let Some(publisher) = publisher.filter(|publisher| !publisher.is_empty()) {
        plan.stamp_petal(&petal_id, &verse_id, publisher, import_hlc as u64)?;
    }
    plan.finish();

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

    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(petal_id.clone()),
        op_type: OpType::ImportPetal,
        payload: json!({ "petal_id": petal_id, "source_node_id": source_node_id, "import_id": import_id }),
        sig: "00".repeat(64),
    };
    if let Err(error) = commit_operation(db, entry, |_lamport| async { Ok::<(), anyhow::Error>(()) }).await {
        tracing::warn!(petal_id = %petal_id, "petal snapshot restored but its ImportPetal op-log entry failed: {error:#}");
    }
    Ok(ImportedPetal {
        petal_id,
        node_ids,
        roles_not_imported,
        verse_id,
        actor: actor.map(str::to_owned),
        import_id,
    })
}

/// DB-thread reply to `DbCommand::ImportPetalSnapshot`, echoing `correlation_id`; local only (no petal clock stamps).
pub async fn import_petal_snapshot_reply(
    db: &Db,
    rows: PetalSnapshotRows,
    correlation_id: Option<String>,
    actor: Option<String>,
) -> DbResult {
    import_petal_snapshot_reply_as(db, None, None, rows, correlation_id, actor).await.0
}

/// DB-thread reply for a host that replicates: `publisher` (this host's DID) clocks the petal row and the importer
/// passes `door`. Send the reply, then hand the [`ImportedPetal`] to [`publish_imported_petal_through`].
pub(crate) async fn import_petal_snapshot_reply_as(
    db: &Db,
    door: Option<&mut NodeWriteDoor>,
    publisher: Option<&str>,
    rows: PetalSnapshotRows,
    correlation_id: Option<String>,
    actor: Option<String>,
) -> (DbResult, Option<ImportedPetal>) {
    let petal_id = rows.petal_id();
    let outcome =
        import_petal_snapshot_correlated(db, door, rows, correlation_id.as_deref(), actor.as_deref(), publisher).await;
    let reply = match outcome {
        Ok(imported) => {
            return (
                DbResult::PetalSnapshotImported {
                    petal_id,
                    correlation_id,
                },
                Some(imported),
            )
        }
        // Refusals carry their reason; nothing was written for any of them.
        Err(error @ (ImportError::PetalExists | ImportError::Collision | ImportError::Invalid(_) | ImportError::Unattributed)) => {
            DbResult::PetalSnapshotImportRefused {
                petal_id,
                reason: error.to_string(),
                correlation_id,
            }
        }
        // A budget refusal keeps its `node write budget exceeded; retry after <ms> ms` text (fe-api answers 429).
        Err(error) => correlated_failure(&correlation_id, format!("ImportPetalSnapshot {petal_id} failed: {error}")),
    };
    (reply, None)
}

/// After commit: publish the petal row, then each node through the ordinary scene publisher (journal proof,
/// own-version stamps, proven asset dependencies), charging the importer at `budget`. Anything not enqueued is left
/// to replica-open recovery.
pub async fn publish_imported_petal(
    db: &Db,
    budget: &mut ImportWriteBudget,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    imported: &ImportedPetal,
) -> ImportPublication {
    publish_imported_petal_through(db, &mut budget.door, blobs, sender, local_did, imported).await
}

/// [`publish_imported_petal`] at the DB thread's own write door (handlers/AGENTS.md §write-budget "Imports").
pub(crate) async fn publish_imported_petal_through(
    db: &Db,
    door: &mut NodeWriteDoor,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    imported: &ImportedPetal,
) -> ImportPublication {
    let mut publication = ImportPublication::default();
    let Some(sender) = sender else {
        // Nothing is sent now: the importer still pays for the journal facts the import stored.
        charge_unpublished(db, door, imported).await;
        publication.nodes_deferred = imported.node_ids.len();
        return publication;
    };
    charge_restored_history(db, door, imported).await;
    // The parent first, so receivers do not have to park every node behind a missing petal.
    match super::local_catalog::publish_petal(db, blobs, Some(sender), local_did, &imported.petal_id).await {
        Ok(()) => publication.petal_enqueued = true,
        Err(error) => tracing::warn!(petal_id = %imported.petal_id, "imported petal publish deferred: {error:#}"),
    }
    for (index, node_id) in imported.node_ids.iter().enumerate() {
        // A full queue would refuse every remaining row: stop, recovery republishes them from the same durable facts.
        if sender.is_full() {
            for deferred in &imported.node_ids[index..] {
                door.charge_committed(db, deferred).await;
            }
            publication.nodes_deferred += imported.node_ids.len() - index;
            tracing::warn!(petal_id = %imported.petal_id, "replication queue full; recovery publishes the remaining imported nodes");
            break;
        }
        // The ordinary post-commit publisher, charged like a local write: the import fact and the published row.
        door.publish(db, blobs, Some(sender), local_did, node_id).await;
        publication.nodes_attempted += 1;
    }
    tracing::info!(
        petal_id = %imported.petal_id,
        petal_enqueued = publication.petal_enqueued,
        nodes_attempted = publication.nodes_attempted,
        nodes_deferred = publication.nodes_deferred,
        "imported petal published after commit; enqueued is not delivery"
    );
    publication
}

/// Charge the importer for everything an import stored when none of it is published now (history and import facts).
async fn charge_unpublished(db: &Db, door: &mut NodeWriteDoor, imported: &ImportedPetal) {
    charge_restored_history(db, door, imported).await;
    for node_id in &imported.node_ids {
        door.charge_committed(db, node_id).await;
    }
}

/// [`import_petal_snapshot_reply`] on a replicating host with no replication sender (the DB thread arm when
/// offline): `publisher` clocks the petal, the importer passes `budget` and is charged what the import stored.
pub async fn import_petal_snapshot_reply_charged(
    db: &Db,
    budget: &mut ImportWriteBudget,
    publisher: &str,
    rows: PetalSnapshotRows,
    correlation_id: Option<String>,
    actor: Option<String>,
) -> DbResult {
    let (reply, imported) =
        import_petal_snapshot_reply_as(db, Some(&mut budget.door), Some(publisher), rows, correlation_id, actor).await;
    if let Some(imported) = imported {
        charge_unpublished(db, &mut budget.door, &imported).await;
    }
    reply
}

/// Charge the importer for the archive journal facts the import restored (CBOR bytes as stored).
async fn charge_restored_history(db: &Db, door: &mut NodeWriteDoor, imported: &ImportedPetal) {
    let Some(actor) = imported.actor.as_deref() else {
        return;
    };
    if imported.node_ids.is_empty() || imported.verse_id.is_empty() {
        return;
    }
    let measured = async {
        let mut response = db
            .query(format!(
                "SELECT VALUE bytes::len(encoding::cbor::encode($this)) FROM node_log WHERE node_id IN $import_nodes AND payload.{ARCHIVE_FACT_KEY} = $import_id"
            ))
            .bind(("import_nodes", imported.node_ids.clone()))
            .bind(("import_id", imported.import_id.clone()))
            .await?
            .check()?;
        let sizes: Vec<Value> = response.take(0)?;
        Ok::<u64, anyhow::Error>(sizes.iter().filter_map(Value::as_u64).sum())
    }
    .await;
    match measured {
        Ok(bytes) => door.charge_bytes(&imported.verse_id, actor, usize::try_from(bytes).unwrap_or(usize::MAX)),
        Err(error) => tracing::debug!(petal_id = %imported.petal_id, "import history could not be measured: {error:#}"),
    }
}

/// Replica-open scene recovery of one verse (the `ReemitLocalVerseCatalog` path): `(examined, enqueued)` node rows.
pub async fn recover_verse_scene_rows(
    db: &Db,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    verse_id: &str,
) -> anyhow::Result<(usize, usize)> {
    let report = super::scene_publication::recover(db, blobs, sender, local_did, verse_id).await?;
    Ok((report.examined, report.enqueued))
}

/// What one import stamps on every quarantined op-log row it writes.
struct Provenance<'a> {
    import_id: &'a str,
    correlation_id: Option<&'a str>,
    /// Importer DID; provenance only, never an authorization input.
    actor: Option<&'a str>,
}

/// Publisher the import's publishable check assumes when the caller names neither publisher nor actor.
const IMPORT_CHECK_PUBLISHER: &str = "did:key:import-check";

/// Longest actor string stamped on quarantined history.
const MAXIMUM_ACTOR_LENGTH: usize = 256;

/// The validated restore transaction: script, its bindings and the highest restored clock per family.
struct RestorePlan {
    /// Guards first (they must see only pre-existing rows), then the writes.
    script: String,
    /// Write statements, appended after every guard when the plan is finished.
    writes: String,
    bindings: Vec<(String, Value)>,
    /// Max op-log `lamport_clock`, node-log `hlc_timestamp` and reading `hlc_timestamp`.
    clock_maxima: [u64; 3],
    /// Imported node ids in snapshot order.
    node_ids: Vec<String>,
    /// The petal's replicated columns as the archive carries them (the keys the import clocks).
    petal_projection: Value,
    /// Each imported node's normalized journal snapshot, for the publishable check after validation.
    projected_nodes: Vec<(String, Value)>,
    /// The archive's enforced node property rules at the petal scope (`field_def` rows), applied to imported values.
    property_rules: Vec<Value>,
}

/// Columns of a petal row that replicate (`field_clock::publish::row_select("petal")`); only these are clocked.
const PETAL_REPLICATED_COLUMNS: [&str; 13] = [
    "petal_id", "fractal_id", "name", "node_id", "bounds", "created_at", "description", "visibility", "tags",
    "hexon_manifest", "terrain", "visibility_groups", "render_distance",
];

/// Largest imported petal row: a published clocked row carries each own value twice and must fit `MAX_ROW_BYTES`.
const MAX_IMPORTED_PETAL_BYTES: usize = fe_sdk::property_schema::MAX_NODE_BYTES;

impl RestorePlan {
    /// Validate every row against the snapshot petal and its scope, render one `BEGIN … COMMIT` script; writes nothing.
    fn build(
        petal_id: &str,
        petal_scope: &str,
        provenance: &Provenance<'_>,
        rows: PetalSnapshotRows,
    ) -> Result<Self, ImportError> {
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
            node_ids: Vec::new(),
            petal_projection: Value::Null,
            projected_nodes: Vec::new(),
            property_rules: Vec::new(),
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
            "IF {petal_tables} OR array::len((SELECT VALUE id FROM op_log_imported WHERE import_petal_id = ${petal_binding})) > 0 OR array::len((SELECT VALUE id FROM field_def WHERE scope = ${scope_binding} OR string::starts_with(scope, ${scope_child_binding}))) > 0 {{ THROW '{PETAL_EXISTS_MARKER}' }}"
        ));
        let inside_petal_scope = |scope: &str| scope == petal_scope || scope.starts_with(&scope_child);

        let petal_record = record_id(&petal, "petal")?.to_string();
        // Live write-time rules a replicated petal row must meet: the row bound and the terrain writer's check.
        if serde_json::to_vec(&petal).map_or(true, |bytes| bytes.len() > MAX_IMPORTED_PETAL_BYTES) {
            return Err(invalid(format!("the petal row exceeds {MAX_IMPORTED_PETAL_BYTES} bytes")));
        }
        if petal.get("terrain").is_some_and(|terrain| {
            !terrain.is_null() && !super::replicated_row::is_valid_inbound_petal_terrain(terrain)
        }) {
            return Err(invalid("the petal terrain fails the terrain writer's check"));
        }
        plan.petal_projection = Value::Object(
            petal
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .filter(|(column, value)| PETAL_REPLICATED_COLUMNS.contains(&column.as_str()) && !value.is_null())
                        .map(|(column, value)| (column.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        );
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
        // (node_id, x binding, z binding, carries properties) for the import journal facts.
        let mut node_facts: Vec<(String, String, String, bool)> = Vec::new();
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
            // The live write-time node bound, on the same normalized form; refused here, never stored and deferred forever.
            let mut sized = node.clone();
            if let Some(object) = sized.as_object_mut() {
                object.remove("id");
            }
            let normalized = super::node_history::normalize_snapshot(sized)
                .and_then(|normalized| super::node_history::check_snapshot_size(&normalized).map(|()| normalized))
                .map_err(|error| invalid(format!("node {node_id}: {error:#}")))?;
            plan.projected_nodes.push((node_id.clone(), normalized));
            let has_properties = node.get("properties").is_some_and(|properties| !properties.is_null());
            if let Some(asset_id) = string_field(&node, "asset_id") {
                referenced_assets.insert(asset_id.to_string());
            }
            let position = take_field(&mut node, "position")
                .as_ref()
                .and_then(point_coordinates)
                .ok_or_else(|| invalid(format!("node {node_id} has no finite GeoJSON point position")))?;
            // Absent elevation is the schema default; any other value must be a number.
            let elevation = match node.get("elevation") {
                None | Some(Value::Null) => 0.0,
                Some(value) => value
                    .as_f64()
                    .ok_or_else(|| invalid(format!("node {node_id} has a non-numeric elevation")))?,
            };
            let (Some(rotation), Some(scale)) = (number_array(&node, "rotation"), number_array(&node, "scale")) else {
                return Err(invalid(format!("node {node_id} has no numeric rotation and scale")));
            };
            if !is_admissible_transform([position.0, elevation, position.1], &rotation, &scale) {
                return Err(invalid(format!("node {node_id} has a transform outside the object-creation bounds")));
            }
            let x = plan.bind(json!(position.0));
            let z = plan.bind(json!(position.1));
            let statement = plan.create_statement(&record, node, Some(format!("{{ position: <geometry<point>> [${x}, ${z}] }}")));
            plan.push(statement);
            plan.node_ids.push(node_id.clone());
            node_facts.push((node_id.clone(), x, z, has_properties));
            node_ids.insert(node_id);
        }

        let mut last_versions: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for mut entry in node_log {
            let record = record_id(&entry, "node_log")?.to_string();
            let Some(node_id) = string_field(&entry, "node_id").filter(|node_id| node_ids.contains(*node_id)) else {
                return Err(invalid("a node_log row names a node outside the snapshot"));
            };
            let version = entry.get("row_version").and_then(Value::as_i64).unwrap_or(0);
            let last = last_versions.entry(node_id.to_string()).or_insert(0);
            *last = (*last).max(version);
            observe_clock(&mut plan.clock_maxima[1], entry.get("hlc_timestamp"), "node_log hlc_timestamp")?;
            // Archive history is local history only, never publication proof: the proof site refuses this marker.
            let payload = entry
                .as_object_mut()
                .ok_or_else(|| invalid("a node_log row is not an object"))?
                .entry("payload")
                .or_insert_with(|| json!({}));
            payload
                .as_object_mut()
                .ok_or_else(|| invalid("a node_log row has a non-object payload"))?
                .insert(ARCHIVE_FACT_KEY.into(), json!(provenance.import_id));
            let statement = plan.create_statement(&record, entry, None);
            plan.push(statement);
        }

        // One fresh local journal fact per node, written by the importer at the import clock: the same baseline a
        // local create writes, and the publication proof `scene_publication::prepare` requires. Its state is read
        // back from the row just written, so it equals `node_history::normalize_snapshot` of the stored row.
        // Without an actor the import stays local (no proof): see AGENTS.md §petal-snapshot-replication.
        if provenance.actor.is_some() {
            for (index, (node_id, x, z, has_properties)) in node_facts.into_iter().enumerate() {
                let version = last_versions
                    .get(&node_id)
                    .copied()
                    .unwrap_or(0)
                    .max(0)
                    .checked_add(1)
                    .ok_or_else(|| invalid(format!("node {node_id} has an exhausted history version")))?;
                let node_binding = plan.bind(json!(node_id));
                let log_binding = plan.bind(json!(ulid::Ulid::new().to_string()));
                let version_binding = plan.bind(json!(version));
                let properties = if has_properties { String::new() } else { ", properties: {}".to_string() };
                plan.push(format!(
                    "LET $fact_state_{index} = array::first((SELECT * OMIT id FROM node WHERE node_id = ${node_binding}))"
                ));
                plan.push(format!(
                    "LET $fact_after_{index} = object::extend($fact_state_{index}, {{ position: [${x}, $fact_state_{index}.elevation, ${z}]{properties} }})"
                ));
                plan.push(format!(
                    "CREATE node_log CONTENT {{ log_id: ${log_binding}, node_id: ${node_binding}, hlc_timestamp: $import_fact_hlc, op: 'baseline', source_did: $import_fact_actor, row_version: ${version_binding}, created_at: $import_fact_at, payload: {{ journal_version: {JOURNAL_VERSION}, scope: $import_fact_scope, coverage_reason: '{IMPORT_COVERAGE_REASON}', state: $fact_after_{index}, after_state: $fact_after_{index} }} }}"
                ));
            }
        }

        let mut field_def_ids: HashSet<String> = HashSet::new();
        for field_def in field_defs {
            let record = record_id(&field_def, "field_def")?.to_string();
            if !string_field(&field_def, "scope").is_some_and(inside_petal_scope) {
                return Err(invalid("a field_def row is scoped outside the snapshot petal"));
            }
            let field_def_id = string_field(&field_def, "field_def_id")
                .filter(|field_def_id| !field_def_id.is_empty())
                .ok_or_else(|| invalid("a field_def row has no field_def_id"))?;
            if !field_def_ids.insert(field_def_id.to_string()) {
                return Err(invalid(format!("field_def_id {field_def_id} appears twice in the snapshot")));
            }
            // The live schema writer's parse (size cap MAX_SCHEMA_BYTES and rule shape).
            if let Some(descriptor) = field_def.get("enforced_schema").filter(|value| !value.is_null()) {
                fe_sdk::property_schema::EnforcedPropertySchema::parse(descriptor)
                    .map_err(|error| invalid(format!("field_def {field_def_id}: {error}")))?;
                // A petal-scope node rule governs the imported nodes as it will govern their later live writes.
                if string_field(&field_def, "scope") == Some(petal_scope) && string_field(&field_def, "entity_type") == Some("node") {
                    if let Some(key) = string_field(&field_def, "key") {
                        plan.property_rules.push(json!({
                            "field_def_id": field_def_id, "scope": petal_scope, "key": key, "enforced_schema": descriptor,
                        }));
                    }
                }
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

        for mut asset in assets {
            // Archive-supplied owner, size and provenance are never trusted (src/handlers/AGENTS.md "Snapshot import trust").
            if let Some(object) = asset.as_object_mut() {
                object.remove("owner_did");
                object.remove("publication_provenance");
                object.remove(fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER);
                // An anchor naming another petal is pinned to the imported petal, never trusted.
                if object.get("petal_id").is_some_and(|anchor| !anchor.is_null()) {
                    object.insert("petal_id".into(), json!(petal_id));
                }
                if object.get("size_bytes").and_then(Value::as_i64).is_none_or(|size| size < 0) {
                    object.insert("size_bytes".into(), json!(0));
                }
                // An attributed import is the uploader of the assets it introduces (a live upload's owner and anchor),
                // which is the publication proof their bytes need; a pre-existing asset row is never touched (below).
                if let Some(actor) = provenance.actor {
                    object.insert("owner_did".into(), json!(actor));
                    object.insert("petal_id".into(), json!(petal_id));
                }
            }
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
        let mut source_ids: HashSet<String> = HashSet::new();
        for mut entry in oplog {
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
            let lamport = entry
                .get("lamport_clock")
                .filter(|clock| clock.as_u64().is_some_and(|clock| clock > 0));
            if lamport.is_none() {
                return Err(invalid("an op_log entry has no positive lamport_clock"));
            }
            observe_clock(&mut plan.clock_maxima[0], lamport, "op_log lamport_clock")?;
            if !source_ids.insert(record.clone()) {
                return Err(invalid(format!("op_log record {record} appears twice in the snapshot")));
            }
            // Quarantine: imported history lands in `op_log_imported`, which no proof or marker lookup reads.
            let Some(object) = entry.as_object_mut() else {
                return Err(invalid("an op_log entry is not an object"));
            };
            object.remove("id");
            object.insert("source_id".into(), json!(record));
            object.insert("import_petal_id".into(), json!(petal_id));
            object.insert("import_id".into(), json!(provenance.import_id));
            // Remove first: a forged archive value must never survive.
            object.remove("imported_by");
            if let Some(actor) = provenance.actor.filter(|actor| !actor.is_empty() && actor.len() <= MAXIMUM_ACTOR_LENGTH) {
                object.insert("imported_by".into(), json!(actor));
            }
            match provenance.correlation_id {
                Some(correlation_id) => object.insert("correlation_id".into(), json!(correlation_id)),
                None => object.remove("correlation_id"),
            };
            let row_binding = plan.bind(crate::repo::strip_json_nulls(entry));
            plan.push(format!("CREATE op_log_imported CONTENT ${row_binding}"));
        }

        // Identity guards (security inbox 5, 8): every imported node and reading identity must be new here, so a
        // snapshot can never take over or shadow another scope's rows. Op-log markers need no guard: quarantined.
        let mut node_identities = node_ids;
        node_identities.extend(dangling_node_ids);
        // node_log outlives a hard petal delete, so an id with surviving history is as taken as a live node.
        plan.collision_guard(|set| format!("SELECT VALUE id FROM node_log WHERE node_id IN ${set}"), node_identities.clone());
        plan.collision_guard(|set| format!("SELECT VALUE id FROM node WHERE node_id IN ${set}"), node_identities);
        plan.collision_guard(|set| format!("SELECT VALUE id FROM iot_reading WHERE reading_id IN ${set}"), reading_ids);
        plan.collision_guard(|set| format!("SELECT VALUE id FROM field_def WHERE field_def_id IN ${set}"), field_def_ids);
        Ok(plan)
    }

    /// Append the writes after every guard and close the transaction; call once, last.
    fn finish(&mut self) {
        let writes = std::mem::take(&mut self.writes);
        self.script.push_str(&writes);
        self.script.push_str("COMMIT TRANSACTION;");
    }

    /// Bind `value` under a fresh `import_<n>` name and return that name.
    fn bind(&mut self, value: Value) -> String {
        let name = format!("import_{}", self.bindings.len());
        self.bindings.push((name.clone(), value));
        name
    }

    /// Bind `value` under a fixed name the build's statements already reference.
    fn bind_named(&mut self, name: &str, value: Value) {
        self.bindings.push((name.to_string(), value));
    }

    /// Clock every replicated petal key as `publisher`'s own version at `hlc`, in this transaction: the facts
    /// `field_clock::publish` would stamp on a first publish of a row with no versions (field_clock/AGENTS.md §publish).
    fn stamp_petal(&mut self, petal_id: &str, verse_id: &str, publisher: &str, hlc: u64) -> Result<(), ImportError> {
        let versions: Vec<crate::field_clock::FieldVersion> = crate::field_clock::keys_of("petal", &self.petal_projection)
            .into_iter()
            .map(|(field, value)| crate::field_clock::FieldVersion {
                field,
                author: publisher.to_string(),
                hlc,
                value: Some(value),
                deleted: false,
            })
            .collect();
        if versions.is_empty() {
            return Ok(());
        }
        let winners: Vec<&crate::field_clock::FieldVersion> = versions.iter().collect();
        let stamp = |error: anyhow::Error| invalid(format!("the petal row cannot be clocked: {error:#}"));
        let (mut statements, mut bindings) =
            crate::field_clock::versions_write_sql("petal", petal_id, verse_id, &versions).map_err(stamp)?;
        let (clock_statements, clock_bindings) =
            crate::field_clock::clock_write_sql("petal", petal_id, &winners).map_err(stamp)?;
        statements.extend(clock_statements);
        bindings.extend(clock_bindings);
        for statement in statements {
            self.push(statement);
        }
        self.bindings.extend(bindings);
        Ok(())
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

/// Every element of an array field as `f64`; `None` when absent or any element is not a number.
fn number_array(row: &Value, field: &str) -> Option<Vec<f64>> {
    row.get(field)?.as_array()?.iter().map(Value::as_f64).collect()
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

