use fe_query::{Filter, InsertBuilder, QueryBuilder};

use crate::op_log::next_hlc_timestamp;
use crate::query_helpers::exec_query;
use crate::repo::Db;

/// Append an immutable INSERT-only entry to `node_log` — single-writer
/// invariant required (see fe-database/src/AGENTS.md §node-log).
pub(crate) async fn append_node_log(
    db: &Db,
    node_id: &str,
    op: &str,
    source_did: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let (hlc_packed, _hlc_str) = next_hlc_timestamp();
    let log_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Derive next row_version for this node (append-only monotonic counter)
    let rv_q = QueryBuilder::new()
        .select(&["math::max(row_version) AS max_rv"])
        .from("node_log")
        .filter(Filter::eq("node_id", node_id))
        .group_all()
        .build();
    let mut rv_res = exec_query(db, &rv_q)
        .await
        .map_err(|e| anyhow::anyhow!("node_log row_version query failed: {e}"))?;
    let rv_rows: Vec<serde_json::Value> = rv_res.take(0).unwrap_or_default();
    let next_rv = rv_rows
        .first()
        .and_then(|r| r["max_rv"].as_i64())
        .unwrap_or(0)
        + 1;

    let insert_q = InsertBuilder::insert_into("node_log")
        .values(serde_json::json!({
            "log_id": log_id,
            "node_id": node_id,
            "hlc_timestamp": hlc_packed as i64,
            "op": op,
            "source_did": source_did,
            "payload": payload,
            "row_version": next_rv,
            "created_at": now,
        }))
        .build();
    exec_query(db, &insert_q)
        .await
        .map_err(|e| anyhow::anyhow!("node_log INSERT failed: {e}"))?;

    Ok(())
}
