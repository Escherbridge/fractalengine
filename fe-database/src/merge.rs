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
    /// The row's non-transform fields merged, but the local transform won the
    /// `edit_seq` register (QA-0100), so position/rotation/scale stayed local.
    AppliedKeptLocalTransform,
    /// The payload was not a node row (no `node_id`) — ignored.
    NotANode,
    /// A markerless live row lost the legacy-writer precedence to the node's
    /// recorded markerless author (QA-0156) — nothing changed.
    SkippedLegacyPrecedence,
}

/// The author of a markerless (legacy, clockless) node row, ranked by the
/// legacy-writer precedence: the verse owner first, then the larger DID
/// (QA-0156; see AGENTS.md §lifecycle "legacy writer precedence").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LegacyAuthor<'a> {
    pub is_owner: bool,
    pub did: &'a str,
}

/// Transform columns that form one last-writer-wins register keyed by
/// `(edit_seq, transform value)`; see AGENTS.md §lifecycle "transform register".
const TRANSFORM_FIELDS: [&str; 5] = ["position", "elevation", "rotation", "scale", "edit_seq"];

/// The register's comparison key: `edit_seq`, then the flattened transform
/// `[x, z, elevation, rotation.., scale..]` as a deterministic tie-break.
fn transform_key(edit_seq: i64, position: Option<(f64, f64)>, row: &serde_json::Value) -> (i64, Vec<f64>) {
    let (x, z) = position.unwrap_or((0.0, 0.0));
    let mut values = vec![x, z, row.get("elevation").and_then(|v| v.as_f64()).unwrap_or(0.0)];
    for field in ["rotation", "scale"] {
        if let Some(items) = row.get(field).and_then(|v| v.as_array()) {
            values.extend(items.iter().map(|item| item.as_f64().unwrap_or(0.0)));
        }
    }
    (edit_seq, values)
}

/// Whether `incoming` beats `local`: higher `edit_seq`, else the larger value
/// vector under `f64::total_cmp`. Every peer evaluates the same pair the same way.
fn incoming_transform_wins(incoming: &(i64, Vec<f64>), local: &(i64, Vec<f64>)) -> bool {
    match incoming.0.cmp(&local.0) {
        std::cmp::Ordering::Equal => {
            let ordering = incoming
                .1
                .iter()
                .zip(local.1.iter())
                .map(|(a, b)| a.total_cmp(b))
                .find(|o| o.is_ne())
                .unwrap_or_else(|| incoming.1.len().cmp(&local.1.len()));
            ordering.is_gt()
        }
        other => other.is_gt(),
    }
}

/// The local row's transform register key, or `None` when the row is absent.
async fn read_local_transform_key(db: &Db, node_id: &str) -> anyhow::Result<Option<(i64, Vec<f64>)>> {
    let mut response = db
        .query("SELECT edit_seq, position, elevation, rotation, scale FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("merge transform lookup failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("merge transform lookup statement failed: {e}"))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("merge transform lookup take failed: {e}"))?;
    Ok(rows.first().map(|row| {
        let mut row = row.clone();
        let seq = row.get("edit_seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let position = extract_point_2d(&mut row);
        transform_key(seq, position, &row)
    }))
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
///
/// Geometry-safe against the real SCHEMAFULL schema (§geometry-inserts): the
/// `position` column is extracted from the payload and written with an
/// explicit `<geometry<point>>` cast — never bound raw through
/// CONTENT/MERGE — and a CREATE backfills the schema-required core fields
/// (a minimal remote tombstone for a node we never had carries only
/// `node_id`/`petal_id`/`tombstone`).
pub async fn apply_replicated_node(db: &Db, row_json: &[u8]) -> anyhow::Result<MergeApplied> {
    apply_replicated_node_by(db, row_json, None).await
}

/// [`apply_replicated_node`] for a row whose author is known: a live row edits
/// an existing node only when `author` ranks at or above the node's recorded
/// markerless writer, so cross-author markerless rows converge to one winner in
/// any arrival order (QA-0156). `None` keeps the blind merge (local reconcile).
pub async fn apply_replicated_node_by(
    db: &Db,
    row_json: &[u8],
    author: Option<LegacyAuthor<'_>>,
) -> anyhow::Result<MergeApplied> {
    let mut incoming: serde_json::Value = serde_json::from_slice(row_json)
        .map_err(|e| anyhow::anyhow!("apply_replicated_node: bad row JSON: {e}"))?;
    let Some(node_id) = incoming.get("node_id").and_then(|v| v.as_str()) else {
        return Ok(MergeApplied::NotANode);
    };
    let node_id = node_id.to_string();

    let incoming_tombstoned = incoming
        .get("tombstone")
        .map(|t| !t.is_null())
        .unwrap_or(false);

    // Geometry column: extract before any CONTENT/MERGE bind (§geometry-inserts).
    let position = extract_point_2d(&mut incoming);

    let (exists, local_tombstoned) = read_local_tombstoned(db, &node_id).await?;

    // Non-resurrection: a local delete wins over a stale live write (D-A7/N-4).
    if local_tombstoned && !incoming_tombstoned {
        tracing::debug!("apply_replicated_node: refusing to resurrect tombstoned {node_id}");
        return Ok(MergeApplied::SkippedTombstoned);
    }

    // Incoming tombstone converges the local row to deleted (merge-safe).
    if incoming_tombstoned {
        if exists {
            let marker = incoming
                .get("tombstone")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            // Two deleters' markers settle on the larger one, never the last to arrive (QA-0156).
            if local_tombstoned && !marker_outranks(&marker, &read_local_tombstone(db, &node_id).await?) {
                return Ok(MergeApplied::AppliedTombstone);
            }
            db.query("UPDATE node SET tombstone = $ts WHERE node_id = $nid")
                .bind(("ts", marker))
                .bind(("nid", node_id.clone()))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("merge apply-tombstone failed: {e}"))?;
        } else {
            // Schema-required core first (inline geometry cast), then the
            // payload's remaining fields — including the tombstone itself —
            // merge onto the fresh row.
            create_core_node(db, &incoming, &node_id, position).await?;
            merge_remaining_fields(db, &incoming, &node_id).await?;
        }
        return Ok(MergeApplied::AppliedTombstone);
    }

    // Live row, local not tombstoned — field-merge if present, else create.
    if exists {
        if let Some(author) = author {
            if read_legacy_writer(db, &node_id).await?.is_some_and(|(is_owner, did)| author < LegacyAuthor { is_owner, did: &did }) {
                tracing::debug!(node_id, author = author.did, "markerless row from a lower-ranked author than the node's legacy writer ignored (QA-0156)");
                return Ok(MergeApplied::SkippedLegacyPrecedence);
            }
        }
        // QA-0100: the transform is an LWW register, never arrival-ordered. A row
        // without `edit_seq` is a legacy writer and keeps the blind merge.
        let mut kept_local_transform = false;
        if let Some(incoming_seq) = incoming.get("edit_seq").and_then(|v| v.as_i64()) {
            if let Some(local_key) = read_local_transform_key(db, &node_id).await? {
                let incoming_key = transform_key(incoming_seq, position, &incoming);
                if !incoming_transform_wins(&incoming_key, &local_key) {
                    kept_local_transform = true;
                    if let Some(object) = incoming.as_object_mut() {
                        for field in TRANSFORM_FIELDS {
                            object.remove(field);
                        }
                    }
                }
            }
        }
        let position = if kept_local_transform { None } else { position };
        db.query("UPDATE node MERGE $row WHERE node_id = $nid")
            .bind(("row", incoming.clone()))
            .bind(("nid", node_id.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("merge update failed: {e}"))?;
        if let Some((x, z)) = position {
            db.query("UPDATE node SET position = <geometry<point>> [$x, $z] WHERE node_id = $nid")
                .bind(("x", x))
                .bind(("z", z))
                .bind(("nid", node_id.clone()))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("merge position cast failed: {e}"))?;
        }
        if let Some(author) = author {
            record_legacy_writer(db, &node_id, author).await?;
        }
        if kept_local_transform {
            return Ok(MergeApplied::AppliedKeptLocalTransform);
        }
    } else {
        create_core_node(db, &incoming, &node_id, position).await?;
        merge_remaining_fields(db, &incoming, &node_id).await?;
        if let Some(author) = author {
            record_legacy_writer(db, &node_id, author).await?;
        }
    }
    Ok(MergeApplied::Applied)
}

/// Deterministic order on tombstone markers: their JSON text with object keys sorted.
fn marker_outranks(incoming: &serde_json::Value, local: &serde_json::Value) -> bool {
    fn canonical(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Object(object) => {
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort();
                let fields: Vec<String> = keys
                    .into_iter()
                    .map(|key| format!("{}:{}", serde_json::Value::from(key.as_str()), canonical(&object[key])))
                    .collect();
                format!("{{{}}}", fields.join(","))
            }
            serde_json::Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
            other => other.to_string(),
        }
    }
    canonical(incoming) > canonical(local)
}

/// The local node's tombstone marker (`null` when absent).
async fn read_local_tombstone(db: &Db, node_id: &str) -> anyhow::Result<serde_json::Value> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT VALUE tombstone FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("merge tombstone lookup failed: {e}"))?
        .take(0)?;
    Ok(rows.into_iter().next().unwrap_or(serde_json::Value::Null))
}

/// The node's recorded markerless writer `(is_owner, did)`, if any.
async fn read_legacy_writer(db: &Db, node_id: &str) -> anyhow::Result<Option<(bool, String)>> {
    let lookup = db
        .query("SELECT is_owner, author FROM node_legacy_writer WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?
        .check();
    let rows: Vec<serde_json::Value> = match lookup {
        Ok(mut response) => response.take(0)?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("legacy writer lookup failed: {e}")),
    };
    Ok(rows.first().and_then(|row| {
        Some((row.get("is_owner")?.as_bool()?, row.get("author")?.as_str()?.to_string()))
    }))
}

async fn record_legacy_writer(db: &Db, node_id: &str, author: LegacyAuthor<'_>) -> anyhow::Result<()> {
    db.query("UPSERT node_legacy_writer SET node_id = $nid, author = $author, is_owner = $owner WHERE node_id = $nid")
        .bind(("nid", node_id.to_string()))
        .bind(("author", author.did.to_string()))
        .bind(("owner", author.is_owner))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("legacy writer record failed: {e}"))?;
    Ok(())
}

/// Extract + remove the `position` geometry column from a node payload.
/// Accepts the two wire shapes a serialized node row can carry: the GeoJSON
/// Point Surreal returns from a `geometry<point>` column, or a plain
/// `[x, z]` array.
fn extract_point_2d(row: &mut serde_json::Value) -> Option<(f64, f64)> {
    let obj = row.as_object_mut()?;
    let pos = obj.remove("position")?;
    let coords = match &pos {
        serde_json::Value::Array(a) if a.len() >= 2 => Some(a),
        serde_json::Value::Object(o) => o.get("coordinates").and_then(|c| c.as_array()),
        _ => None,
    }?;
    let x = coords.first().and_then(|v| v.as_f64())?;
    let z = coords.get(1).and_then(|v| v.as_f64())?;
    Some((x, z))
}

/// Create the schema-required core of a node row with an **inline geometry
/// cast** (§geometry-inserts) — the hand-written-statement pattern of the
/// crud handlers. A `CREATE … CONTENT $row` bind cannot express the cast,
/// and omitting `position`/`rotation`/`scale`/`created_at` fails the
/// SCHEMAFULL type check, so absent rows go through this statement first.
async fn create_core_node(
    db: &Db,
    incoming: &serde_json::Value,
    node_id: &str,
    position: Option<(f64, f64)>,
) -> anyhow::Result<()> {
    let petal_id = incoming
        .get("petal_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if incoming.get("petal_id").and_then(|v| v.as_str()).is_none() {
        tracing::warn!(
            node_id,
            "replicated node row lacks petal_id — creating with an empty scope"
        );
    }
    let (x, z) = position.unwrap_or((0.0, 0.0));
    let created_at = incoming
        .get("created_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    db.query(
        "CREATE node SET \
         node_id = $nid, \
         petal_id = $pid, \
         position = <geometry<point>> [$x, $z], \
         rotation = [0.0, 0.0, 0.0, 1.0], \
         scale = [1.0, 1.0, 1.0], \
         created_at = $created",
    )
    .bind(("nid", node_id.to_string()))
    .bind(("pid", petal_id))
    .bind(("x", x))
    .bind(("z", z))
    .bind(("created", created_at))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("merge core-create failed: {e}"))?;
    Ok(())
}

/// Merge the payload's non-geometry fields onto an existing row (the core
/// statement has just created it).
async fn merge_remaining_fields(
    db: &Db,
    incoming: &serde_json::Value,
    node_id: &str,
) -> anyhow::Result<()> {
    // Strip nulls first: SurrealDB `option<T>` rejects explicit `null`
    // (absence means NONE — same contract as `Repo`'s create paths).
    let mut row = incoming.clone();
    if let Some(obj) = row.as_object_mut() {
        obj.retain(|_, v| !v.is_null());
    }
    db.query("UPDATE node MERGE $row WHERE node_id = $nid")
        .bind(("row", row))
        .bind(("nid", node_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("merge create-fields failed: {e}"))?;
    Ok(())
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
