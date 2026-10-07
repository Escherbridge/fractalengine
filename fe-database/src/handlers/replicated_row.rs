//! Inbound replicated-row apply (A4) — the DB thread's single-writer
//! enforcement point for rows arriving over a verse replica.
//!
//! Loop prevention is **structural**: this handler takes no replication
//! sender, so an applied row can never re-enter the outbound bridge — the
//! dispatch arm is the only caller and it passes none either.
//!
//! TODO(F3/A3): the fe-policy role gate (Viewer denied / Editor+ applied /
//! unknown peer denied, deny-by-default, roles resolved from the `role`
//! table at verse scope — never wire-supplied) slots in at the top of
//! [`apply_replicated_row_handler`] once F3 wires peer-role resolution.

use fe_runtime::messages::{NodeDto, ReplicatedRowOutcome, SceneChange};

use crate::merge::{apply_replicated_node, MergeApplied};
use crate::repo::Db;

/// The DB thread's scene-change seam (entity_change_tx).
pub type SceneChangeSender = tokio::sync::broadcast::Sender<SceneChange>;

/// Apply one inbound replicated row to the durable store (A4).
///
/// The row arrives from the sync thread's inbound pump as the payload bytes
/// the authoring peer published. Geometry columns (`node.position`,
/// `petal.bounds`) are stripped before any CONTENT/MERGE bind and written
/// back with explicit SurrealQL casts (§geometry-inserts); tombstone
/// dominance is delegated to [`crate::merge::apply_replicated_node`] (N-4).
///
/// `verse_id` / `author_did` are audit context for logs (the role gate that
/// will consume `author_did` is F3's seam). `pub` so the test harness's
/// simplified DB loop can drive the **real** apply path against its in-memory
/// DB (the real dispatch loop lives in fe-database's own thread and is not
/// callable against `Mem` — statement-parity isn't enough here, the apply
/// logic itself must be the production one).
#[allow(clippy::too_many_arguments)]
pub async fn apply_replicated_row_handler(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    tracing::debug!(
        verse_id,
        table,
        record_id,
        author = author_did,
        "Applying inbound replicated row"
    );
    match table {
        "verse" | "fractal" | "petal" => apply_static_row(db, table, record_id, row_bytes).await,
        "node" => apply_node_row(db, record_id, row_bytes, entity_change_tx).await,
        other => {
            tracing::debug!(
                table = other,
                "Inbound row for a table the replicated path does not handle"
            );
            Ok(ReplicatedRowOutcome::NotApplicable)
        }
    }
}

// ---------------------------------------------------------------------------
// Static hierarchy tables (verse / fractal / petal)
// ---------------------------------------------------------------------------

/// Insert-or-merge one replicated row into a static hierarchy table.
///
/// Geometry columns are extracted first (never bound through CONTENT/MERGE —
/// §geometry-inserts) and cast-written once the row exists.
async fn apply_static_row(
    db: &Db,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<ReplicatedRowOutcome> {
    let (id_field, geometry_field) = match table {
        "verse" => ("verse_id", None),
        "fractal" => ("fractal_id", None),
        "petal" => ("petal_id", Some("bounds")),
        _ => unreachable!("apply_static_row only receives the static hierarchy tables"),
    };

    let mut row: serde_json::Value = serde_json::from_slice(row_bytes)
        .map_err(|e| anyhow::anyhow!("replicated row {table}/{record_id}: bad JSON: {e}"))?;
    strip_toplevel_nulls(&mut row);

    // Extract the geometry column now, write it with an explicit cast after
    // the row exists (a raw GeoJSON bind against a SCHEMAFULL table fails).
    let geometry_value = match geometry_field {
        None => None,
        Some(field) => row.as_object_mut().and_then(|o| o.remove(field)),
    };

    // Prefer the payload's own id field; fall back to the wire record id.
    let row_id = row
        .get(id_field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| record_id.to_string());
    if row_id != record_id {
        tracing::warn!(
            table,
            record_id,
            row_id = %row_id,
            "Replicated row id differs from the wire key — trusting the payload"
        );
    }

    if row_exists(db, table, id_field, &row_id).await? {
        db.query(format!("UPDATE {table} MERGE $row WHERE {id_field} = $rid").as_str())
            .bind(("row", row.clone()))
            .bind(("rid", row_id.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("replicated {table} merge failed: {e}"))?;
    } else {
        db.query(format!("CREATE {table} CONTENT $row").as_str())
            .bind(("row", row.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("replicated {table} create failed: {e}"))?;
    }

    if let Some(geometry) = geometry_value {
        let field = geometry_field.expect("geometry_value implies a geometry field");
        db.query(
            format!(
                "UPDATE {table} SET {field} = <geometry<polygon>> $geo WHERE {id_field} = $rid"
            )
            .as_str(),
        )
        .bind(("geo", geometry))
        .bind(("rid", row_id))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated {table} geometry cast failed: {e}"))?;
    }

    Ok(ReplicatedRowOutcome::Applied)
}

/// Whether a row with `row_id` exists in `table`. Tolerates an absent table
/// (fresh peer DB before the schema applies) — same shape as merge.rs.
async fn row_exists(db: &Db, table: &str, id_field: &str, row_id: &str) -> anyhow::Result<bool> {
    let res = db
        .query(format!("SELECT id FROM {table} WHERE {id_field} = $rid LIMIT 1").as_str())
        .bind(("rid", row_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("replicated {table} lookup failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match res {
        Ok(mut r) => r
            .take(0)
            .map_err(|e| anyhow::anyhow!("replicated {table} lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("replicated {table} lookup failed: {e}")),
    };
    Ok(!rows.is_empty())
}

// ---------------------------------------------------------------------------
// Node table — tombstone-honoring, geometry-safe
// ---------------------------------------------------------------------------

/// Apply one replicated node row: tombstone dominance **and geometry-safe
/// writes** via [`apply_replicated_node`] (N-4 / §geometry-inserts — see
/// merge.rs), plus a petal-scoped `SceneChange` for live applies and
/// tombstone applies (A4).
async fn apply_node_row(
    db: &Db,
    record_id: &str,
    row_bytes: &[u8],
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    let mut row: serde_json::Value = serde_json::from_slice(row_bytes)
        .map_err(|e| anyhow::anyhow!("replicated node row {record_id}: bad JSON: {e}"))?;

    // merge.rs keys on the payload's `node_id`; fall back to the wire id.
    let node_id = row
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| record_id.to_string());
    if let Some(obj) = row.as_object_mut() {
        obj.insert("node_id".to_string(), serde_json::json!(node_id.clone()));
    }
    let petal_id = row
        .get("petal_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let payload = serde_json::to_vec(&row)?;
    let outcome = match apply_replicated_node(db, &payload).await? {
        MergeApplied::NotANode => return Ok(ReplicatedRowOutcome::NotApplicable),
        MergeApplied::SkippedTombstoned => ReplicatedRowOutcome::SkippedTombstoned,
        MergeApplied::AppliedTombstone => {
            if let Some(petal_id) = petal_id {
                emit_scene_change(
                    entity_change_tx,
                    SceneChange::NodeRemoved {
                        node_id: node_id.clone(),
                        petal_id,
                    },
                );
            }
            ReplicatedRowOutcome::AppliedTombstone
        }
        MergeApplied::Applied => {
            if let Some(petal_id) = petal_id {
                emit_scene_change(
                    entity_change_tx,
                    SceneChange::NodeAdded {
                        node: node_dto_from_row(&row, &node_id, &petal_id),
                    },
                );
            }
            ReplicatedRowOutcome::Applied
        }
    };
    Ok(outcome)
}

/// Read the payload's `position` as `(x, z)` for the scene-change DTO.
/// Accepts the GeoJSON Point Surreal returns from a `geometry<point>`
/// column or a plain `[x, z]` array (read-only twin of merge.rs's extractor).
fn point_2d_of(row: &serde_json::Value) -> Option<(f64, f64)> {
    let coords = match row.get("position")? {
        serde_json::Value::Array(a) if a.len() >= 2 => Some(a),
        serde_json::Value::Object(o) => o.get("coordinates").and_then(|c| c.as_array()),
        _ => None,
    }?;
    let x = coords.first().and_then(|v| v.as_f64())?;
    let z = coords.get(1).and_then(|v| v.as_f64())?;
    Some((x, z))
}

/// Build the petal-scoped [`NodeDto`] for a live node apply's scene change.
fn node_dto_from_row(row: &serde_json::Value, node_id: &str, petal_id: &str) -> NodeDto {
    let name = row
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(node_id)
        .to_string();
    let elevation = row.get("elevation").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (x, z) = point_2d_of(row).unwrap_or((0.0, 0.0));
    let asset_id = row.get("asset_id").and_then(|v| v.as_str());
    NodeDto {
        node_id: node_id.to_string(),
        petal_id: petal_id.to_string(),
        name,
        position: [x as f32, elevation as f32, z as f32],
        rotation: extract_f32_array(row.get("rotation"), [0.0, 0.0, 0.0, 1.0]),
        scale: extract_f32_array(row.get("scale"), [1.0, 1.0, 1.0]),
        has_asset: asset_id.is_some(),
        // The blob bytes do not ride the row; the asset arrives via the
        // blob-transfer seam and is linked by `asset_id`.
        asset_path: None,
    }
}

/// Read a fixed-size `f32` array field with a default for absent/short values.
fn extract_f32_array<const N: usize>(
    value: Option<&serde_json::Value>,
    default: [f32; N],
) -> [f32; N] {
    let mut out = [0.0f32; N];
    let mut filled = 0usize;
    if let Some(arr) = value.and_then(|v| v.as_array()) {
        for (slot, item) in out.iter_mut().zip(arr.iter()) {
            if let Some(f) = item.as_f64() {
                *slot = f as f32;
                filled += 1;
            }
        }
    }
    if filled == N {
        out
    } else {
        default
    }
}

/// Best-effort scene-change send — a closed channel is shutdown, not an error.
fn emit_scene_change(tx: Option<&SceneChangeSender>, change: SceneChange) {
    if let Some(tx) = tx {
        if tx.send(change).is_err() {
            tracing::warn!("Scene-change channel closed — scene subscriber may have shut down");
        }
    }
}

/// Strip top-level JSON nulls: SurrealDB `option<T>` rejects explicit `null`,
/// and absence means `NONE` (same contract as `Repo`'s create paths, §repo).
fn strip_toplevel_nulls(row: &mut serde_json::Value) {
    if let Some(obj) = row.as_object_mut() {
        obj.retain(|_, v| !v.is_null());
    }
}

// ---------------------------------------------------------------------------
// Tests — durable apply READ-BACK against the real schema (A4)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh in-memory DB with the **real schema** applied (the production
    /// table shapes, geometry columns included) so READ-BACK assertions
    /// exercise the same SCHEMAFULL constraints as the live store.
    async fn schema_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema apply");
        db
    }

    async fn select_json(db: &Db, stmt: &str) -> Vec<serde_json::Value> {
        let mut res = db.query(stmt).await.unwrap();
        res.take::<Vec<serde_json::Value>>(0).unwrap()
    }

    #[tokio::test]
    async fn verse_row_applies_and_reads_back() {
        let db = schema_db().await;
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-e2e",
            "name": "Replicated Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "namespace_id": null,
            "default_access": "viewer",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-e2e",
            "verse",
            "verse-e2e",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK (not handler-Ok): the durable store holds the row.
        let rows = select_json(
            &db,
            "SELECT verse_id, name FROM verse WHERE verse_id = 'verse-e2e'",
        )
        .await;
        assert_eq!(rows.len(), 1, "verse row must be durably present");
        assert_eq!(rows[0]["name"], "Replicated Verse");
    }

    #[tokio::test]
    async fn verse_row_merge_updates_existing() {
        let db = schema_db().await;
        let first = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "First",
            "created_by": "did:key:a", "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
        }))
        .unwrap();
        apply_replicated_row_handler(&db, "v1", "verse", "v1", &first, "did:key:a", None)
            .await
            .unwrap();

        let update = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "Renamed by peer",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v1", &update, "did:key:peer-b", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT name, created_by FROM verse WHERE verse_id = 'v1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Renamed by peer", "MERGE updates fields");
        assert_eq!(
            rows[0]["created_by"], "did:key:a",
            "MERGE preserves untouched fields"
        );
    }

    #[tokio::test]
    async fn node_row_with_geojson_position_applies_geometry_safe() {
        let db = schema_db().await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);
        // Position arrives as the GeoJSON Point Surreal serializes from a
        // geometry<point> column.
        let row = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-geo-1",
            "petal_id": "petal-1",
            "display_name": "Replicated Node",
            "position": { "type": "Point", "coordinates": [4.5, -2.5] },
            "elevation": 12.0,
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "interactive": false,
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-geo-1",
            &row,
            "did:key:peer-a",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the geometry column holds a real point (a raw GeoJSON
        // bind would have failed the SCHEMAFULL write entirely).
        let rows = select_json(
            &db,
            "SELECT position, elevation, display_name FROM node WHERE node_id = 'node-geo-1'",
        )
        .await;
        assert_eq!(rows.len(), 1, "node row must be durably present");
        assert_eq!(rows[0]["elevation"], 12.0);
        let coords = rows[0]["position"]["coordinates"].as_array().unwrap();
        assert_eq!(coords[0].as_f64().unwrap(), 4.5);
        assert_eq!(coords[1].as_f64().unwrap(), -2.5);

        // A4: the node apply emits the petal-scoped SceneChange.
        match scene_rx.try_recv().expect("NodeAdded scene change") {
            SceneChange::NodeAdded { node } => {
                assert_eq!(node.node_id, "node-geo-1");
                assert_eq!(node.petal_id, "petal-1");
                assert_eq!(node.position, [4.5, 12.0, -2.5]);
            }
            other => panic!("expected NodeAdded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn node_tombstone_row_emits_node_removed() {
        let db = schema_db().await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);
        let row = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-del-1",
            "petal_id": "petal-1",
            "tombstone": { "hlc": 42, "source_did": "did:key:peer-a" },
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del-1",
            &row,
            "did:key:peer-a",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);

        match scene_rx.try_recv().expect("NodeRemoved scene change") {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "node-del-1");
                assert_eq!(petal_id, "petal-1");
            }
            other => panic!("expected NodeRemoved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unhandled_table_is_not_applicable() {
        let db = schema_db().await;
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "role",
            "whatever",
            br#"{"role":"editor"}"#,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::NotApplicable);
    }
}
