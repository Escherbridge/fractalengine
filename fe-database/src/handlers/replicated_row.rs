//! Inbound replicated-row apply (A4) — the DB thread's single-writer
//! enforcement point for rows arriving over a verse replica.
//!
//! Loop prevention is **structural**: this handler takes no replication
//! sender, so an applied row can never re-enter the outbound bridge — the
//! dispatch arm is the only caller and it passes none either.
//!
//! A3: admission is role-gated here (deny-by-default). The author's role is
//! resolved from the local tables at the verse scope — never from the wire —
//! and must reach Editor+ (the same fe-policy standard write gate as the
//! local path). See [`admit_inbound_row`].

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
/// `author_did` (the entry author the transport attests) is what the A3
/// role gate resolves against the local tables — wire payloads are never
/// consulted for roles. `pub` so the test harness's
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
    // A3: admission precedes dispatch — a denied row is never applied and
    // never re-emitted (there is no replication sender in scope anyway).
    match admit_inbound_row(db, verse_id, table, record_id, row_bytes, author_did).await? {
        Admission::Allow => {}
        Admission::Deny(reason) => {
            tracing::warn!(
                verse_id,
                table,
                record_id,
                author = author_did,
                reason = %reason,
                "Inbound replicated row DENIED at the verse-scope role gate — not applied"
            );
            return Ok(ReplicatedRowOutcome::Denied);
        }
    }
    match table {
        "verse" | "fractal" | "petal" => {
            // An empty entry is the iroh-docs `del` marker. Static hierarchy
            // rows have no tombstone semantics in this model (N-4 covers
            // nodes) — a peer-side delete of a verse/petal is not a thing we
            // apply, so report it as not-applicable rather than an error.
            if row_bytes.is_empty() {
                tracing::debug!(
                    table,
                    record_id,
                    "empty-entry delete on a static hierarchy table — no tombstone semantics"
                );
                return Ok(ReplicatedRowOutcome::NotApplicable);
            }
            apply_static_row(db, table, record_id, row_bytes).await
        }
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
// A3: the inbound role gate
// ---------------------------------------------------------------------------

/// Verdict of the inbound admission gate.
enum Admission {
    /// The row may be applied.
    Allow,
    /// The row is denied — never applied, never re-emitted.
    Deny(String),
}

/// A3: gate one inbound row on the author's role at the verse scope.
///
/// Deny-by-default, roles resolved from the **local** tables, never the
/// wire: [`crate::role_manager::resolve_role`] walks the verse owner
/// (`created_by`) → explicit `role` rows up the scope chain → the verse's
/// `default_access`. Nothing pins `default_access` to a safe value at the
/// schema level — the column carries only a `DEFAULT 'viewer'`
/// (schema.rs), no `ASSERT` — so the safe values are enforced where they
/// are written: `set_default_access` (local path) and `apply_static_row`
/// (inbound replicated manifests, which reject anything outside
/// `"viewer"`/`"none"`) together keep an unknown peer from ever resolving
/// to a writer through this table. The decision itself is
/// [`crate::rbac::evaluate_write`] — the same fe-policy standard write gate
/// as the local path (Write = Editor+), so inbound and local writes can
/// never drift apart in threshold.
///
/// **Bootstrap window:** rows for a verse this store does not know yet are
/// admitted. A peer only receives rows over a replica it deliberately opened
/// (namespace capability in hand) — that capability is the admission until
/// the verse manifest converges and real roles become resolvable. Denying
/// here would deadlock convergence at the root: the verse row itself, which
/// the gate keys on, arrives over this same path (the A2 two-peer flow
/// depends on this). The window closes the moment the manifest lands; every
/// row after it is role-gated. A capability-holder could always plant a
/// manifest first anyway, so this rule gates nothing extra — it only keeps
/// out-of-order sync (petal rows arriving before the verse row) from being
/// permanently lost.
///
/// **Cross-verse referential confinement (known gap, future hardening):**
/// this gate confines a row to the replica's verse (`verse_id` match for
/// verse rows; the manifest's scope for interior rows) but does not verify
/// that an admitted interior row's *references* (a node row's `petal_id`,
/// a petal row's `fractal_id`, …) belong to that same verse — an Editor
/// admitted in verse A could push rows referencing verse B's ids. Full
/// referential-confinement checking is recorded on the conductor board (the
/// p2p_mycelium_completion FUTURE-HARDENING note) and belongs at this
/// admission gate once the verse-manifest/role bootstrap contract is
/// specified; it is deliberately out of M1 scope.
async fn admit_inbound_row(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<Admission> {
    // A verse row claiming a different verse than the replica it arrived on
    // is a scope-injection attempt: the gate would judge the replica's verse
    // while the row lands under the payload's (which, pre-manifest, would
    // also slip through the bootstrap window unchecked). Never trust it.
    if table == "verse" {
        if let Some(claimed) = serde_json::from_slice::<serde_json::Value>(row_bytes)
            .ok()
            .and_then(|row| {
                row.get("verse_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
        {
            if claimed != verse_id {
                tracing::warn!(
                    verse_id,
                    claimed_verse_id = %claimed,
                    record_id,
                    author = author_did,
                    "verse row claims a foreign verse_id — denied"
                );
                return Ok(Admission::Deny(
                    "verse_id does not match the replica's verse".to_string(),
                ));
            }
        }
    }

    // Bootstrap window (see the doc comment above).
    if !row_exists(db, "verse", "verse_id", verse_id).await? {
        tracing::debug!(
            verse_id,
            table,
            record_id,
            "verse manifest not converged yet — bootstrap admission"
        );
        return Ok(Admission::Allow);
    }

    let verse_scope = crate::scope::build_scope(verse_id, None, None);
    let role = crate::role_manager::resolve_role(db, author_did, &verse_scope).await?;
    match crate::rbac::evaluate_write(author_did, &role.to_string(), &verse_scope) {
        fe_policy::Decision::Allow => Ok(Admission::Allow),
        fe_policy::Decision::Deny(reason) => Ok(Admission::Deny(reason)),
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

    // A3 deny-by-default (F20 fold-in): an inbound verse manifest is the
    // only writer that could hand an unknown peer a write role — `resolve_role`
    // falls back to `default_access` for peers with no explicit row, and the
    // schema only DEFAULTs the column (no ASSERT). The local writers are
    // pinned ("viewer" at creation, `set_default_access` rejects anything
    // else), so the replicated path validates the same set here: a manifest
    // carrying anything outside {"viewer", "none"} is denied outright, never
    // applied.
    if table == "verse" {
        if let Some(access) = row.get("default_access") {
            let valid = access
                .as_str()
                .is_some_and(|v| v == "viewer" || v == "none");
            if !valid {
                tracing::warn!(
                    verse_id = record_id,
                    default_access = ?access,
                    "Inbound verse manifest carries a forbidden default_access — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
    }

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
        // §geometry-inserts, strictest form: a cast around a BOUND GeoJSON
        // object — or even around an object literal with a bound rings
        // parameter — is rejected by the schema check ("could not cast into
        // `geometry`"). The only form this SurrealDB line casts is the inline
        // numeric literal the local petal paths use (crud.rs, seed.rs). So:
        // extract the payload's coordinate rings and render them inline.
        // Only validated finite floats are formatted — no injection surface.
        let rings = geometry.get("coordinates").cloned().ok_or_else(|| {
            anyhow::anyhow!("replicated {table} row {row_id}: bounds lacks coordinates rings")
        })?;
        let literal = render_polygon_rings(&rings).ok_or_else(|| {
            anyhow::anyhow!(
                "replicated {table} row {row_id}: bounds coordinates are not numeric rings"
            )
        })?;
        db.query(
            format!(
                "UPDATE {table} SET {field} = \
                 <geometry<polygon>> {{ type: 'Polygon', coordinates: {literal} }} \
                 WHERE {id_field} = $rid"
            )
            .as_str(),
        )
        .bind(("rid", row_id))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated {table} geometry cast failed: {e}"))?;
    }

    Ok(ReplicatedRowOutcome::Applied)
}

/// Render a GeoJSON `coordinates` value as an inline SurrealQL numeric
/// literal (`[[[x, z], …]]`) — the only statement form this SurrealDB line
/// accepts under a `<geometry<polygon>>` cast (see apply_static_row). Every
/// rendered token comes from a validated finite f64, so the interpolation
/// carries no injection surface; `None` on any non-numeric shape.
fn render_polygon_rings(coords: &serde_json::Value) -> Option<String> {
    let mut out = String::from("[");
    for (ri, ring) in coords.as_array()?.iter().enumerate() {
        if ri > 0 {
            out.push(',');
        }
        out.push('[');
        for (pi, point) in ring.as_array()?.iter().enumerate() {
            if pi > 0 {
                out.push(',');
            }
            let pair = point.as_array()?;
            let x = pair.first()?.as_f64()?;
            let z = pair.get(1)?.as_f64()?;
            if !x.is_finite() || !z.is_finite() {
                return None;
            }
            out.push_str(&format!("[{x}, {z}]"));
        }
        out.push(']');
    }
    out.push(']');
    Some(out)
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
    // An empty payload is the wire tombstone — the iroh-docs `del` marker's
    // empty entry (`RowChange::is_tombstone` mirrors this at the seam).
    // Synthesize the node payload the merge path expects; the petal scope
    // for the scene change is resolved from the durable row below, since
    // the wire form carries nothing.
    let mut row: serde_json::Value = if row_bytes.is_empty() {
        serde_json::json!({
            "node_id": record_id,
            "tombstone": { "source": "remote-del" },
        })
    } else {
        serde_json::from_slice(row_bytes)
            .map_err(|e| anyhow::anyhow!("replicated node row {record_id}: bad JSON: {e}"))?
    };

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
            // The empty-entry tombstone carries no petal — resolve the owning
            // petal from the durable row for the scene change (§scene-change
            // attribution: an absent lookup suppresses the event rather than
            // broadcasting an unscoped delta).
            let petal_id = match petal_id {
                Some(p) => Some(p),
                None => node_petal_id(db, &node_id).await,
            };
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

/// The durable row's owning petal, for scene-change attribution when the
/// inbound payload carries none (the empty-entry tombstone form). Best-effort:
/// a failed or absent lookup yields `None`, which suppresses the scene event
/// (§scene-change attribution) rather than broadcasting an unscoped delta.
async fn node_petal_id(db: &Db, node_id: &str) -> Option<String> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .ok()?
        .check()
        .ok()?
        .take(0)
        .ok()?;
    rows.first()
        .and_then(|r| r.get("petal_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
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
        // Fresh store: the verse manifest itself arrives over the replica —
        // bootstrap admission (see admit_inbound_row), the A2 convergence case.
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
        // The verse creator (created_by) — resolves to Owner, so the update
        // passes the A3 gate. The unknown-peer update is covered by
        // `unknown_peer_verse_update_is_denied`.
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
            "verse_id": "v1", "name": "Renamed by owner",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v1", &update, "did:key:a", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT name, created_by FROM verse WHERE verse_id = 'v1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Renamed by owner", "MERGE updates fields");
        assert_eq!(
            rows[0]["created_by"], "did:key:a",
            "MERGE preserves untouched fields"
        );
    }

    /// F20 fold-in (A3 deny-by-default): an inbound verse manifest carrying a
    /// `default_access` outside {viewer, none} is denied at the replicated
    /// write path — without this, a capability-holding peer could plant
    /// `default_access: "editor"` and make every unknown peer resolve to a
    /// writer for that verse (the schema column has only a DEFAULT, no ASSERT).
    #[tokio::test]
    async fn inbound_verse_manifest_with_editor_default_access_is_denied() {
        let db = schema_db().await;
        // Fresh store: the gate's bootstrap window admits the manifest (this
        // is exactly how a verse converges on a joining peer — A2), so the
        // default_access validation is what must reject it.
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-hijack",
            "name": "Hijacked Access Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "editor",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-hijack",
            "verse",
            "verse-hijack",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        // READ-BACK: the manifest never landed — unknown peers cannot gain a
        // write role through this verse.
        let rows = select_json(
            &db,
            "SELECT verse_id FROM verse WHERE verse_id = 'verse-hijack'",
        )
        .await;
        assert!(
            rows.is_empty(),
            "forbidden default_access manifest must not be applied"
        );
    }

    /// `none` is a valid inbound `default_access` (deny-by-default in the
    /// strictest form) — the validation must not reject safe values.
    #[tokio::test]
    async fn inbound_verse_manifest_with_none_default_access_applies() {
        let db = schema_db().await;
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-none",
            "name": "Private Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "none",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-none",
            "verse",
            "verse-none",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT verse_id, default_access FROM verse WHERE verse_id = 'verse-none'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["default_access"], "none");
    }

    #[tokio::test]
    async fn node_row_with_geojson_position_applies_geometry_safe() {
        let db = schema_db().await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);
        // No verse row is seeded — this row rides the bootstrap window (the
        // gated variant is `editor_role_row_applies_and_reads_back`).
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

    // -------------------------------------------------------------------
    // A3: the inbound role gate — deny-by-default, roles from local tables
    // -------------------------------------------------------------------

    /// Seed a verse row (owner = `created_by`) plus optional explicit role
    /// rows at the verse scope, exactly as the production paths write them.
    async fn seed_verse_with_roles(db: &Db, verse_id: &str, roles: &[(&str, &str)]) {
        let _: Option<serde_json::Value> = db
            .create("verse")
            .content(serde_json::json!({
                "verse_id": verse_id,
                "name": "Gated Verse",
                "created_by": "did:key:owner-a",
                "created_at": "2026-10-07T00:00:00Z",
                "default_access": "viewer",
            }))
            .await
            .expect("seed verse");
        let scope = crate::scope::build_scope(verse_id, None, None);
        for (did, role) in roles {
            crate::rbac::assign_role(db, did, &scope, role)
                .await
                .expect("seed role");
        }
    }

    fn gated_node_row(node_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "node_id": node_id,
            "petal_id": "petal-1",
            "display_name": "Gated Node",
            "position": { "type": "Point", "coordinates": [1.5, 2.5] },
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn editor_role_row_applies_and_reads_back() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-1",
            &gated_node_row("node-gated-1"),
            "did:key:peer-ed",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the Editor peer's row is durable.
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        // A4 travels with admission: the gated apply still emits the
        // petal-scoped scene change.
        assert!(matches!(
            scene_rx.try_recv(),
            Ok(SceneChange::NodeAdded { .. })
        ));
    }

    #[tokio::test]
    async fn viewer_role_row_is_denied_never_applied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-view", "viewer")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-2",
            &gated_node_row("node-gated-2"),
            "did:key:peer-view",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        // READ-BACK: the Viewer's row never touched the durable store…
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-2'",
        )
        .await;
        assert!(rows.is_empty(), "denied row must not be persisted");
        // …and emitted no scene change (never re-emitted anywhere).
        assert!(scene_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unknown_peer_row_is_denied() {
        let db = schema_db().await;
        // No role row for the author: resolution falls to the verse's
        // default_access ("viewer"), which must not admit a writer.
        seed_verse_with_roles(&db, "v1", &[]).await;

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-3",
            &gated_node_row("node-gated-3"),
            "did:key:peer-unknown",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-3'",
        )
        .await;
        assert!(rows.is_empty(), "unknown peer's row must not be persisted");
    }

    /// F20/M1 finding 2 (reconciliation second chance): a row denied because
    /// its author's role had not yet converged locally must apply when the
    /// same row is **replayed** (the startup reconciliation path re-applies a
    /// replica's snapshot) with the same authenticated author DID — this only
    /// works because live delivery and snapshot replay attribute authorship
    /// identically (fe-sync imports the endpoint identity as the docs
    /// author). Re-delivery denial must stay harmless: the denied replay
    /// leaves no trace.
    #[tokio::test]
    async fn denied_row_converges_on_replay_after_role_lands() {
        let db = schema_db().await;
        // The verse manifest has converged; carol's Editor role row has NOT.
        seed_verse_with_roles(&db, "v1", &[]).await;
        let row = gated_node_row("node-replay-1");

        // First delivery (live): carol resolves to default_access viewer → denied.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-replay-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        assert!(
            select_json(
                &db,
                "SELECT node_id FROM node WHERE node_id = 'node-replay-1'"
            )
            .await
            .is_empty(),
            "denied row must not be persisted"
        );

        // The role row lands (e.g. it replicated later than carol's rows).
        let scope = crate::scope::build_scope("v1", None, None);
        crate::rbac::assign_role(&db, "did:key:carol", &scope, "editor")
            .await
            .expect("seed carol's editor role");

        // Replay (restart reconciliation): the SAME row, the SAME author DID.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-replay-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Applied,
            "the second chance must converge the previously denied row"
        );

        // READ-BACK: converged durably.
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-replay-1'",
        )
        .await;
        assert_eq!(
            rows.len(),
            1,
            "replayed row is durable after the role landed"
        );
    }

    /// Re-delivery of an already-applied row is idempotent-harmless (the
    /// reconciliation pass replays the whole snapshot on every open): the
    /// second apply MERGEs onto the existing row and stays `Applied` — the
    /// "denial on re-delivery must stay harmless" contract for rows authored
    /// under a stale identity.
    #[tokio::test]
    async fn already_applied_row_redelivery_stays_applied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:carol", "editor")]).await;
        let row = gated_node_row("node-idem-1");

        let first = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-idem-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(first, ReplicatedRowOutcome::Applied);

        let second = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-idem-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(second, ReplicatedRowOutcome::Applied);
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-idem-1'",
        )
        .await;
        assert_eq!(rows.len(), 1, "re-delivery does not duplicate the row");
    }

    #[tokio::test]
    async fn unknown_peer_verse_update_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;

        let update = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "Hijacked Name",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v1", &update, "did:key:peer-b", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        // READ-BACK: the verse row is untouched.
        let rows = select_json(&db, "SELECT name FROM verse WHERE verse_id = 'v1'").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Gated Verse");
    }

    #[tokio::test]
    async fn verse_row_claiming_foreign_verse_id_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;

        // The payload claims a different verse than the replica it arrived
        // on — the gate would judge verse v1 while the row lands as v2.
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v2", "name": "Injected Verse",
            "created_by": "did:key:owner-a", "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v2", &row, "did:key:owner-a", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        let rows = select_json(&db, "SELECT verse_id FROM verse WHERE verse_id = 'v2'").await;
        assert!(rows.is_empty(), "foreign verse_id must not be planted");
    }

    #[tokio::test]
    async fn node_row_before_verse_manifest_applies_bootstrap() {
        let db = schema_db().await;
        // Out-of-order sync: interior rows can arrive before the verse
        // manifest converges — they ride the bootstrap window (see
        // admit_inbound_row) instead of being permanently lost.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-boot-1",
            &gated_node_row("node-boot-1"),
            "did:key:peer-any",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-boot-1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
    }

    // -------------------------------------------------------------------
    // A5: tombstone dominance through the real inbound path (N-4)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn stale_live_row_cannot_resurrect_locally_tombstoned_node() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let live = gated_node_row("node-n4");

        // 1. The live node converges.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &live,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // 2. An incoming tombstone converges the local row to deleted.
        let tombstone = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-n4", "petal_id": "petal-1",
            "tombstone": { "hlc": 42, "source_did": "did:key:peer-ed" },
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &tombstone,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-n4'").await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]["tombstone"].is_object(),
            "row must read back tombstoned"
        );

        // 3. A stale live row (a replica that never saw the delete) must NOT
        //    resurrect it — N-4 holds through the real inbound path.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &live,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::SkippedTombstoned);
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-n4'").await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]["tombstone"].is_object(),
            "the tombstone must survive the stale live write"
        );
    }

    #[tokio::test]
    async fn empty_entry_tombstone_converges_local_node_and_emits_scene_change() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        // The node is live locally first.
        apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del",
            &gated_node_row("node-del"),
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();

        // An EMPTY payload is the iroh-docs `del` marker — the wire tombstone
        // form (`RowChange::is_tombstone` at the seam). It must converge the
        // local row to deleted and emit the petal-scoped NodeRemoved, with the
        // petal resolved from the durable row (the wire form carries none).
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del",
            b"",
            "did:key:peer-ed",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);

        // READ-BACK: tombstoned.
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-del'").await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["tombstone"].is_object());

        match scene_rx.try_recv().expect("NodeRemoved scene change") {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "node-del");
                assert_eq!(petal_id, "petal-1");
            }
            other => panic!("expected NodeRemoved, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // A6: petal bounds — the polygon geometry twin of the node point case
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn petal_row_with_bounds_applies_geometry_safe() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let row = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-bounds",
            "fractal_id": "fractal-1",
            "name": "Replicated Petal",
            "node_id": "did:key:peer-ed",
            "bounds": {
                "type": "Polygon",
                "coordinates": [[
                    [-12.0, -8.0], [12.0, -8.0], [12.0, 8.0], [-12.0, 8.0], [-12.0, -8.0]
                ]]
            },
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "petal",
            "petal-bounds",
            &row,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the geometry column holds the real polygon (a raw
        // GeoJSON bind would have failed the SCHEMAFULL write entirely).
        let rows = select_json(
            &db,
            "SELECT bounds FROM petal WHERE petal_id = 'petal-bounds'",
        )
        .await;
        assert_eq!(rows.len(), 1, "petal row must be durably present");
        let bounds = &rows[0]["bounds"];
        assert_eq!(bounds["type"], "Polygon");
        let ring = bounds["coordinates"][0].as_array().unwrap();
        assert_eq!(
            ring.len(),
            5,
            "the polygon ring survives the cast round-trip"
        );
        assert_eq!(ring[0][0].as_f64().unwrap(), -12.0);
        assert_eq!(ring[0][1].as_f64().unwrap(), -8.0);
        assert_eq!(ring[2][0].as_f64().unwrap(), 12.0);
        assert_eq!(ring[2][1].as_f64().unwrap(), 8.0);
    }
}
