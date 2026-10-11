//! `DbCommand::ElevatedQuery` — an Owner's `/api/v1/query/elevated` script run
//! by the single writer: atomic op-log + writes, snapshot diff, then scene
//! broadcast and replication (fe-database/src/AGENTS.md §handlers; fe-api
//! AGENTS.md §elevated-query).

use std::collections::HashMap;

use fe_runtime::messages::{DbResult, SceneChange};

use crate::handlers::replicated_row::{emit_scene_change, node_dto_from_row, SceneChangeSender};
use crate::op_log::{commit_operation_atomic, most_specific_error};
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{BlobStoreHandle, ReplicationSender};

/// DB-side mirror of fe-api's `ELEVATED_TABLES` (the API parse is authoritative).
pub const ELEVATED_WRITABLE_TABLES: [&str; 6] =
    ["node", "fractal", "petal", "field_def", "model", "room"];

/// Upper bound on the rows a write script's tables may hold (the diff reads them all twice).
pub const ELEVATED_DIFF_MAX_ROWS: usize = 20_000;

/// Write tables the inbound replication applier handles, with their domain id field.
const REPLICATED_TABLES: [(&str, &str); 3] = [
    ("node", "node_id"),
    ("fractal", "fractal_id"),
    ("petal", "petal_id"),
];

/// The `ElevatedQuery` command's payload.
pub struct ElevatedRequest {
    pub statements: Vec<String>,
    pub vars: HashMap<String, serde_json::Value>,
    pub write_tables: Vec<String>,
    pub actor_did: String,
    pub correlation_id: String,
}

/// Where a committed write's effects go (all `None`/absent in a bare DB).
pub struct ElevatedSinks<'a> {
    pub blob_store: &'a BlobStoreHandle,
    pub repl_tx: Option<&'a ReplicationSender>,
    pub entity_change_tx: Option<&'a SceneChangeSender>,
}

/// The DB thread's reply to `ElevatedQuery`: `ElevatedQueryResult` with the
/// caller's statements' rows, or `RequestFailed` echoing its id.
pub async fn elevated_query_reply(
    db: &Db,
    sinks: ElevatedSinks<'_>,
    request: ElevatedRequest,
) -> DbResult {
    let outcome = if request.write_tables.is_empty() {
        run_read_only(db, &request).await
    } else {
        run_write(db, &sinks, &request).await
    };
    match outcome {
        Ok((data, changed_rows)) => DbResult::ElevatedQueryResult {
            data,
            changed_rows,
            correlation_id: request.correlation_id,
        },
        Err(error) => DbResult::RequestFailed {
            error,
            correlation_id: request.correlation_id,
        },
    }
}

/// A script that writes nothing: run as-is, never op-logged.
async fn run_read_only(
    db: &Db,
    request: &ElevatedRequest,
) -> Result<(Vec<serde_json::Value>, usize), String> {
    let mut query = db.query(request.statements.join(";\n"));
    for (key, value) in &request.vars {
        query = query.bind((key.clone(), value.clone()));
    }
    let mut response = query.await.map_err(|e| e.to_string())?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        return Err(most_specific_error(errors));
    }
    let mut data = Vec::new();
    for index in 0..response.num_statements() {
        data.extend(
            response
                .take::<Vec<serde_json::Value>>(index)
                .map_err(|e| e.to_string())?,
        );
    }
    Ok((data, 0))
}

/// Durable first (snapshot → atomic commit → diff → `edit_seq` bump →
/// `node_log`), then events (scene changes, replication).
async fn run_write(
    db: &Db,
    sinks: &ElevatedSinks<'_>,
    request: &ElevatedRequest,
) -> Result<(Vec<serde_json::Value>, usize), String> {
    if let Some(table) = request
        .write_tables
        .iter()
        .find(|t| !ELEVATED_WRITABLE_TABLES.contains(&t.as_str()))
    {
        return Err(format!(
            "table '{table}' is not writable through elevated queries"
        ));
    }
    if count_rows(db, &request.write_tables).await? > ELEVATED_DIFF_MAX_ROWS {
        return Err(format!(
            "elevated write over a table larger than {ELEVATED_DIFF_MAX_ROWS} rows is not supported"
        ));
    }
    let before: HashMap<RowKey, Fingerprint> = read_rows(db, &request.write_tables)
        .await?
        .into_iter()
        .map(|(key, row)| {
            let fingerprint = Fingerprint::of(&key.table, &row);
            (key, fingerprint)
        })
        .collect();

    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(request.actor_did.clone()),
        op_type: OpType::ElevatedQuery,
        payload: serde_json::json!({
            "sql": request.statements.join(";\n"),
            "vars": request.vars,
            "actor_did": request.actor_did,
            "write_tables": request.write_tables,
        }),
        sig: "00".repeat(64),
    };
    let bindings = request
        .vars
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let (_, statement_rows) = commit_operation_atomic(db, entry, &request.statements, bindings)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let data: Vec<serde_json::Value> = statement_rows.into_iter().flatten().collect();

    let unreadable =
        |e: String| format!("the write committed but its change set could not be read: {e}");
    let mut after = read_rows(db, &request.write_tables)
        .await
        .map_err(unreadable)?;
    let changes = diff(&before, &after);
    bump_edit_seq(db, &changes, &mut after).await;
    append_node_logs(db, &request.actor_did, &changes, &after).await;
    publish(db, sinks, &changes, &after).await;
    Ok((data, changes.len()))
}

/// One row's identity across the two snapshots: its table + SurrealDB record id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RowKey {
    table: String,
    record: String,
}

/// What the pre-snapshot keeps per row: a content hash, plus a node's scene identity.
struct Fingerprint {
    hash: blake3::Hash,
    node: Option<NodeIdentity>,
}

/// The fields a hard-deleted or newly tombstoned node's `NodeRemoved` needs.
#[derive(Clone)]
struct NodeIdentity {
    node_id: String,
    petal_id: String,
    tombstoned: bool,
}

impl Fingerprint {
    fn of(table: &str, row: &serde_json::Value) -> Self {
        Self {
            hash: row_hash(row),
            node: (table == "node").then(|| node_identity(row)).flatten(),
        }
    }
}

fn row_hash(row: &serde_json::Value) -> blake3::Hash {
    blake3::hash(&serde_json::to_vec(row).unwrap_or_default())
}

fn node_identity(row: &serde_json::Value) -> Option<NodeIdentity> {
    Some(NodeIdentity {
        node_id: row.get("node_id")?.as_str()?.to_string(),
        petal_id: row.get("petal_id")?.as_str()?.to_string(),
        tombstoned: is_tombstoned(row),
    })
}

fn is_tombstoned(row: &serde_json::Value) -> bool {
    row.get("tombstone").is_some_and(|t| !t.is_null())
}

/// A row the script created, changed, or hard-deleted.
enum Change {
    /// Created (`existed: false`) or updated; `was_live_node` = a live node before.
    Upserted {
        key: RowKey,
        existed: bool,
        was_live_node: bool,
    },
    /// Hard-deleted; a node keeps its pre-snapshot identity for `NodeRemoved`.
    Deleted { node: Option<NodeIdentity> },
}

impl Change {
    fn updated_node(&self) -> Option<&RowKey> {
        match self {
            Change::Upserted {
                key, existed: true, ..
            } if key.table == "node" => Some(key),
            _ => None,
        }
    }
}

/// Created/updated rows (new or differently hashed) and hard deletes. Exact:
/// the serial DB thread is the only writer between the two snapshots.
fn diff(
    before: &HashMap<RowKey, Fingerprint>,
    after: &HashMap<RowKey, serde_json::Value>,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for (key, row) in after {
        match before.get(key) {
            Some(old) if old.hash == row_hash(row) => {}
            Some(old) => changes.push(Change::Upserted {
                key: key.clone(),
                existed: true,
                was_live_node: old.node.as_ref().is_some_and(|n| !n.tombstoned),
            }),
            None => changes.push(Change::Upserted {
                key: key.clone(),
                existed: false,
                was_live_node: false,
            }),
        }
    }
    for (key, old) in before {
        if !after.contains_key(key) {
            changes.push(Change::Deleted {
                node: old.node.clone(),
            });
        }
    }
    changes
}

/// `edit_seq += 1` on every updated node (transform.rs precedent), refreshing
/// `after` with the bumped rows so replication carries the new sequence.
async fn bump_edit_seq(
    db: &Db,
    changes: &[Change],
    after: &mut HashMap<RowKey, serde_json::Value>,
) {
    let node_ids: Vec<String> = changes
        .iter()
        .filter_map(Change::updated_node)
        .filter_map(|key| after.get(key)?.get("node_id")?.as_str().map(String::from))
        .collect();
    if node_ids.is_empty() {
        return;
    }
    let bumped: Result<Vec<serde_json::Value>, String> = async {
        db.query("UPDATE node SET edit_seq += 1 WHERE node_id IN $changed RETURN AFTER")
            .bind(("changed", node_ids))
            .await
            .and_then(|r| r.check())
            .map_err(|e| e.to_string())?
            .take(0)
            .map_err(|e| e.to_string())
    }
    .await;
    match bumped {
        Ok(rows) => {
            for row in rows {
                if let Some(record) = row.get("id").map(|id| id.to_string()) {
                    after.insert(
                        RowKey {
                            table: "node".to_string(),
                            record,
                        },
                        row,
                    );
                }
            }
        }
        Err(e) => tracing::warn!("elevated write: edit_seq bump failed: {e}"),
    }
}

/// One `node_log` fact (op `elevated_write`) per changed node, like the typed
/// node handlers' best-effort appends (§node-log).
async fn append_node_logs(
    db: &Db,
    actor_did: &str,
    changes: &[Change],
    after: &HashMap<RowKey, serde_json::Value>,
) {
    for change in changes {
        let (node_id, payload) = match change {
            Change::Upserted { key, existed, .. } if key.table == "node" => {
                let Some(row) = after.get(key) else { continue };
                let Some(node_id) = row.get("node_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mut row = row.clone();
                if let Some(object) = row.as_object_mut() {
                    object.remove("id");
                }
                let kind = if *existed { "updated" } else { "created" };
                (
                    node_id.to_string(),
                    serde_json::json!({ "change": kind, "row": row }),
                )
            }
            Change::Deleted { node: Some(node) } => (
                node.node_id.clone(),
                serde_json::json!({ "change": "deleted", "petal_id": node.petal_id }),
            ),
            _ => continue,
        };
        if let Err(e) = crate::handlers::node_log::append_node_log(
            db,
            &node_id,
            "elevated_write",
            actor_did,
            &payload,
        )
        .await
        {
            tracing::warn!("node_log append failed for {node_id}: {e}");
        }
    }
}

/// Scene changes for node rows, then one replication event per created/updated
/// row of a replicated table (hard deletes are not replicated — §elevated-query).
async fn publish(
    db: &Db,
    sinks: &ElevatedSinks<'_>,
    changes: &[Change],
    after: &HashMap<RowKey, serde_json::Value>,
) {
    for change in changes {
        match change {
            Change::Upserted {
                key, was_live_node, ..
            } if key.table == "node" => {
                let Some(row) = after.get(key) else { continue };
                let Some(now) = node_identity(row) else {
                    continue;
                };
                if !now.tombstoned {
                    emit_scene_change(
                        sinks.entity_change_tx,
                        SceneChange::NodeAdded {
                            node: node_dto_from_row(row, &now.node_id, &now.petal_id),
                        },
                    );
                } else if *was_live_node {
                    emit_scene_change(
                        sinks.entity_change_tx,
                        SceneChange::NodeRemoved {
                            node_id: now.node_id,
                            petal_id: now.petal_id,
                        },
                    );
                }
            }
            Change::Deleted { node: Some(node) } if !node.tombstoned => emit_scene_change(
                sinks.entity_change_tx,
                SceneChange::NodeRemoved {
                    node_id: node.node_id.clone(),
                    petal_id: node.petal_id.clone(),
                },
            ),
            _ => {}
        }
    }

    if sinks.repl_tx.is_none() {
        return;
    }
    let hierarchy = match VerseIndex::load(db).await {
        Ok(index) => index,
        Err(e) => {
            tracing::warn!("elevated write: verse lookup failed, rows not replicated: {e}");
            return;
        }
    };
    for change in changes {
        let Change::Upserted { key, .. } = change else {
            continue;
        };
        let Some((table, id_field)) = REPLICATED_TABLES.iter().find(|(t, _)| *t == key.table)
        else {
            continue;
        };
        let Some(row) = after.get(key) else { continue };
        let Some(record_id) = row.get(*id_field).and_then(|v| v.as_str()) else {
            continue;
        };
        let petal_id = (*table == "node")
            .then(|| {
                row.get("petal_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .flatten();
        let Some(verse_id) = hierarchy.verse_of(table, row) else {
            tracing::warn!(
                table,
                record_id,
                "elevated write: row has no resolvable verse — not replicated"
            );
            continue;
        };
        let mut payload = row.clone();
        if let Some(object) = payload.as_object_mut() {
            object.remove("id");
        }
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        crate::replicate_row_with_petal(
            sinks.repl_tx,
            sinks.blob_store,
            &verse_id,
            table,
            record_id,
            &bytes,
            petal_id,
        );
    }
}

/// petal → fractal → verse lookups for replication routing.
struct VerseIndex {
    petal_fractal: HashMap<String, String>,
    fractal_verse: HashMap<String, String>,
}

impl VerseIndex {
    async fn load(db: &Db) -> Result<Self, String> {
        let mut response = db
            .query(
                "SELECT petal_id, fractal_id FROM petal; SELECT fractal_id, verse_id FROM fractal",
            )
            .await
            .and_then(|r| r.check())
            .map_err(|e| e.to_string())?;
        let pairs = |rows: Vec<serde_json::Value>, from: &str, to: &str| {
            rows.iter()
                .filter_map(|row| {
                    Some((
                        row.get(from)?.as_str()?.to_string(),
                        row.get(to)?.as_str()?.to_string(),
                    ))
                })
                .collect::<HashMap<_, _>>()
        };
        let petals = response.take(0).map_err(|e| e.to_string())?;
        let fractals = response.take(1).map_err(|e| e.to_string())?;
        Ok(Self {
            petal_fractal: pairs(petals, "petal_id", "fractal_id"),
            fractal_verse: pairs(fractals, "fractal_id", "verse_id"),
        })
    }

    fn verse_of(&self, table: &str, row: &serde_json::Value) -> Option<String> {
        let field = |name: &str| row.get(name).and_then(|v| v.as_str());
        let fractal_of_petal = |petal: &str| self.petal_fractal.get(petal).map(String::as_str);
        let fractal = match table {
            "fractal" => return field("verse_id").map(String::from),
            "petal" => field("fractal_id")?,
            "node" => fractal_of_petal(field("petal_id")?)?,
            _ => return None,
        };
        self.fractal_verse.get(fractal).cloned()
    }
}

/// Total rows across `tables` (validated names — safe to interpolate).
async fn count_rows(db: &Db, tables: &[String]) -> Result<usize, String> {
    let script: Vec<String> = tables
        .iter()
        .map(|t| format!("SELECT count() AS n FROM {t} GROUP ALL"))
        .collect();
    let mut response = db
        .query(script.join(";\n"))
        .await
        .and_then(|r| r.check())
        .map_err(|e| e.to_string())?;
    let mut total = 0usize;
    for index in 0..tables.len() {
        let rows: Vec<serde_json::Value> = response.take(index).map_err(|e| e.to_string())?;
        total += rows.first().and_then(|row| row["n"].as_u64()).unwrap_or(0) as usize;
    }
    Ok(total)
}

/// Every row of `tables`, keyed by table + record id.
async fn read_rows(
    db: &Db,
    tables: &[String],
) -> Result<HashMap<RowKey, serde_json::Value>, String> {
    let script: Vec<String> = tables
        .iter()
        .map(|t| format!("SELECT * FROM {t}"))
        .collect();
    let mut response = db
        .query(script.join(";\n"))
        .await
        .and_then(|r| r.check())
        .map_err(|e| e.to_string())?;
    let mut rows = HashMap::new();
    for (index, table) in tables.iter().enumerate() {
        let table_rows: Vec<serde_json::Value> = response.take(index).map_err(|e| e.to_string())?;
        for row in table_rows {
            let Some(record) = row.get("id").map(|id| id.to_string()) else {
                continue;
            };
            rows.insert(
                RowKey {
                    table: table.clone(),
                    record,
                },
                row,
            );
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("elevated")
            .use_db("elevated")
            .await
            .expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");
        db
    }

    fn request(statements: &[&str], write_tables: &[&str]) -> ElevatedRequest {
        ElevatedRequest {
            statements: statements.iter().map(|s| s.to_string()).collect(),
            vars: HashMap::new(),
            write_tables: write_tables.iter().map(|s| s.to_string()).collect(),
            actor_did: "did:key:z6MkOwner".to_string(),
            correlation_id: "c1".to_string(),
        }
    }

    /// A read-only script mixing `LET`, a bounded `SELECT` and a scalar
    /// `RETURN` (valid elevated SQL) answers with every statement's rows.
    #[tokio::test]
    async fn read_only_script_with_let_and_return_answers() {
        let db = mem_db().await;
        db.query("CREATE room CONTENT { petal_id: 'p1', name: 'lobby' }")
            .await
            .expect("seed")
            .check()
            .expect("seed ok");
        let blob_store: BlobStoreHandle =
            Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let reply = elevated_query_reply(
            &db,
            ElevatedSinks {
                blob_store: &blob_store,
                repl_tx: None,
                entity_change_tx: None,
            },
            request(
                &[
                    "LET $wanted = 'lobby'",
                    "SELECT VALUE name FROM room WHERE name = $wanted TIMEOUT 5s",
                    "RETURN $wanted",
                ],
                &[],
            ),
        )
        .await;
        let DbResult::ElevatedQueryResult {
            data, changed_rows, ..
        } = reply
        else {
            panic!("expected ElevatedQueryResult, got {reply:?}");
        };
        assert_eq!(changed_rows, 0);
        assert!(
            data.iter().filter(|v| *v == "lobby").count() >= 2,
            "SELECT and RETURN rows both present: {data:?}"
        );
    }

    /// The diff reads every write-table row twice; past the cap the script is
    /// refused before anything runs (no op-log row, no write).
    #[tokio::test]
    async fn a_write_over_a_table_past_the_diff_cap_is_refused_before_running() {
        let db = mem_db().await;
        db.query(format!(
            "FOR $i IN 0..{} {{ CREATE room CONTENT {{ petal_id: <string> $i, name: 'r' }} }}",
            ELEVATED_DIFF_MAX_ROWS + 1
        ))
        .await
        .expect("seed")
        .check()
        .expect("seed ok");
        let blob_store: BlobStoreHandle =
            Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let reply = elevated_query_reply(
            &db,
            ElevatedSinks {
                blob_store: &blob_store,
                repl_tx: None,
                entity_change_tx: None,
            },
            request(&["DELETE room TIMEOUT 5s"], &["room"]),
        )
        .await;
        let DbResult::RequestFailed {
            error,
            correlation_id,
        } = reply
        else {
            panic!("expected RequestFailed, got {reply:?}");
        };
        assert_eq!(correlation_id, "c1");
        assert!(error.contains("larger than 20000 rows"), "{error}");
        let mut counts = db
            .query("SELECT count() AS n FROM room GROUP ALL; SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .expect("counts");
        let rows: Vec<serde_json::Value> = counts.take(0).expect("room count");
        let logs: Vec<serde_json::Value> = counts.take(1).expect("op_log count");
        assert_eq!(rows[0]["n"], ELEVATED_DIFF_MAX_ROWS + 1);
        assert_eq!(logs[0]["n"], 0, "nothing was op-logged");
    }
}
