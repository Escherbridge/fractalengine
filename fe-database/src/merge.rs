//! Durable P2P merge-apply for replicated node rows (FR-1 / N-4).
//! See fe-database/src/AGENTS.md §lifecycle (tombstone non-resurrection).

use crate::repo::Db;

/// How one replicated node row was reconciled against the durable store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeApplied {
    /// The row was upserted (created or field-merged) locally.
    Applied,
    /// A local tombstone dominated an incoming *live* row — resurrection refused.
    SkippedTombstoned,
    /// The incoming row carried a tombstone; the local node is now tombstoned.
    AppliedTombstone,
    /// The payload was not a node row (no `node_id`) — ignored.
    NotANode,
}

/// Whether the local `node` row for `node_id` is tombstoned. Tolerates an absent
/// `node` table (fresh DB) — treated as "not tombstoned / absent".
async fn read_local_tombstoned(db: &Db, node_id: &str) -> anyhow::Result<(bool, bool)> {
    let lookup = db
        .query("SELECT node_id, tombstone FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("merge lookup query failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match lookup {
        Ok(mut res) => res
            .take(0)
            .map_err(|e| anyhow::anyhow!("merge lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("merge lookup statement failed: {e}")),
    };
    let exists = !rows.is_empty();
    let tombstoned = rows
        .first()
        .and_then(|r| r.get("tombstone"))
        .map(|t| !t.is_null())
        .unwrap_or(false);
    Ok((exists, tombstoned))
}

/// Apply one replicated node row (`row_json`) to the durable node table, honoring
/// tombstones (N-4): a stale replica that still holds a node we have tombstoned
/// must NOT resurrect it. This is the durable counterpart of
/// `fe_entity_store::EntityStore::upsert`'s in-memory tombstone guard, and the
/// function `fe-sync`'s reconciliation drives for inbound rows.
pub async fn apply_replicated_node(db: &Db, row_json: &[u8]) -> anyhow::Result<MergeApplied> {
    let incoming: serde_json::Value = serde_json::from_slice(row_json)
        .map_err(|e| anyhow::anyhow!("apply_replicated_node: bad row JSON: {e}"))?;
    let Some(node_id) = incoming.get("node_id").and_then(|v| v.as_str()) else {
        return Ok(MergeApplied::NotANode);
    };
    let node_id = node_id.to_string();

    let incoming_tombstoned = incoming
        .get("tombstone")
        .map(|t| !t.is_null())
        .unwrap_or(false);

    let (exists, local_tombstoned) = read_local_tombstoned(db, &node_id).await?;

    // Non-resurrection: a local delete wins over a stale live write (D-A7/N-4).
    if local_tombstoned && !incoming_tombstoned {
        tracing::debug!("apply_replicated_node: refusing to resurrect tombstoned {node_id}");
        return Ok(MergeApplied::SkippedTombstoned);
    }

    // Incoming tombstone converges the local row to deleted (merge-safe).
    if incoming_tombstoned {
        let marker = incoming
            .get("tombstone")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if exists {
            db.query("UPDATE node SET tombstone = $ts WHERE node_id = $nid")
                .bind(("ts", marker))
                .bind(("nid", node_id.clone()))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("merge apply-tombstone failed: {e}"))?;
        } else {
            db.query("CREATE node CONTENT $row")
                .bind(("row", incoming.clone()))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("merge create-tombstoned failed: {e}"))?;
        }
        return Ok(MergeApplied::AppliedTombstone);
    }

    // Live row, local not tombstoned — field-merge if present, else create.
    if exists {
        db.query("UPDATE node MERGE $row WHERE node_id = $nid")
            .bind(("row", incoming.clone()))
            .bind(("nid", node_id.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("merge update failed: {e}"))?;
    } else {
        db.query("CREATE node CONTENT $row")
            .bind(("row", incoming.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("merge create failed: {e}"))?;
    }
    Ok(MergeApplied::Applied)
}

// ---------------------------------------------------------------------------
// Tests — durable-path merge non-resurrection (FR-1 / N-4)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB seeded with one `node` row (`tombstoned` = whether it carries
    /// a `tombstone` marker). Uses raw `CREATE`/`UPDATE` (mirrors production) so
    /// the test never touches the process-global HLC — safe as a unit test.
    async fn mem_db_with_node(node_id: &str, petal_id: &str, tombstoned: bool) -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        db.query("CREATE node CONTENT { node_id: $nid, petal_id: $pid, display_name: 'n' }")
            .bind(("nid", node_id.to_string()))
            .bind(("pid", petal_id.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
        if tombstoned {
            db.query(
                "UPDATE node SET tombstone = { hlc: 1, source_did: 'x' } WHERE node_id = $nid",
            )
            .bind(("nid", node_id.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
        }
        db
    }

    async fn visible(db: &Db, node_id: &str) -> usize {
        let mut res = db
            .query("SELECT node_id FROM node WHERE node_id = $nid AND tombstone = NONE")
            .bind(("nid", node_id.to_string()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        rows.len()
    }

    #[tokio::test]
    async fn stale_replica_row_cannot_resurrect_durable_tombstone() {
        // A durably-tombstoned node (soft-deleted row persists).
        let db = mem_db_with_node("n1", "petal-1", true).await;
        assert_eq!(visible(&db, "n1").await, 0, "starts tombstoned/hidden");

        // A stale peer re-sends the still-live row through the merge path.
        let stale = serde_json::to_vec(&serde_json::json!({
            "node_id": "n1", "petal_id": "petal-1", "display_name": "Zombie", "elevation": 9.0,
        }))
        .unwrap();
        let applied = apply_replicated_node(&db, &stale).await.expect("apply");
        assert_eq!(
            applied,
            MergeApplied::SkippedTombstoned,
            "a stale live row must NOT resurrect a tombstoned node"
        );

        // Still soft-deleted: the tombstone-filtered read (scene snapshot /
        // reload) returns nothing — non-resurrection through the durable path.
        assert_eq!(
            visible(&db, "n1").await,
            0,
            "tombstoned node stays hidden after a stale merge (survives reload)"
        );
    }

    #[tokio::test]
    async fn incoming_tombstone_converges_local_live_node() {
        let db = mem_db_with_node("n1", "petal-1", false).await;
        assert_eq!(visible(&db, "n1").await, 1, "starts live");

        let tombstoned_row = serde_json::to_vec(&serde_json::json!({
            "node_id": "n1", "petal_id": "petal-1",
            "tombstone": { "hlc": 42, "source_did": "did:key:z6MkPeer" },
        }))
        .unwrap();
        let applied = apply_replicated_node(&db, &tombstoned_row)
            .await
            .expect("apply");
        assert_eq!(applied, MergeApplied::AppliedTombstone);

        let (_exists, tombstoned) = read_local_tombstoned(&db, "n1").await.unwrap();
        assert!(tombstoned, "a remote tombstone must delete the local node");
        assert_eq!(visible(&db, "n1").await, 0);
    }

    #[tokio::test]
    async fn non_node_payload_is_ignored() {
        let db = mem_db_with_node("n1", "petal-1", false).await;
        let verse_row = serde_json::to_vec(&serde_json::json!({ "verse_id": "v1" })).unwrap();
        assert_eq!(
            apply_replicated_node(&db, &verse_row).await.unwrap(),
            MergeApplied::NotANode
        );
    }
}
