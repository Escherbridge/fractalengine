//! Inbound clocked verse/fractal/petal rows: per-key judge, merge and one-transaction write; see field_clock/AGENTS.md §admission.

use super::replicated_row::{json_eq, lookup_row, present, render_polygon_rings, static_field_violation};
use crate::field_clock::judge::{required_level, VerseView};
use crate::field_clock::store::{self, stored_hlc, SqlFragment};
use crate::field_clock::wire::{self, FieldVersion};
use crate::field_clock::merge;
use crate::repo::Db;
use fe_policy::RoleLevel;
use fe_runtime::messages::{DeferralReason, ReplicatedRowOutcome};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Attempts of one apply when a fence finds the row's clock state moved underneath it.
pub(super) const FENCE_ATTEMPTS: usize = 3;
/// THROW text shared by every fence below ([`is_fence_error`] matches on it).
const FENCE_MESSAGE: &str = "clocked row changed";
/// The row's stored versions must be the ones the merge was folded against.
const VERSIONS_FENCE: &str = "IF array::sort((SELECT VALUE [field_key, author, hlc] FROM row_field_version WHERE tbl=$fe_row_tbl AND rid=$fe_row_rid)) != array::sort($fe_versions_before) { THROW 'clocked row changed'; }";
/// The row's cached winners must be the ones the merge started from.
const CLOCK_FENCE: &str = "IF array::sort((SELECT VALUE [field_key, hlc, author] FROM row_field_clock WHERE tbl=$fe_row_tbl AND rid=$fe_row_rid)) != array::sort($fe_winners_before) { THROW 'clocked row changed'; }";
/// The one key a new hierarchy row cannot be created without (every table's `name` is a required string).
const CREATE_REQUIRED_KEY: &str = "name";

/// Static names of one clocked hierarchy table.
pub(super) struct TableShape {
    pub(super) id_field: &'static str,
    pub(super) geometry: Option<&'static str>,
    /// Non-optional columns: a deleted version of one is a malformed row.
    undeletable: &'static [&'static str],
}

impl TableShape {
    /// The shape of `verse`, `fractal` or `petal`; `None` for any other table.
    pub(super) fn of(table: &str) -> Option<Self> {
        match table {
            "verse" => Some(Self {
                id_field: "verse_id",
                geometry: None,
                undeletable: &["name", "default_access", "ts_mode", "ts_replication_factor", "ts_bucket_width_ms"],
            }),
            "fractal" => Some(Self { id_field: "fractal_id", geometry: None, undeletable: &["name"] }),
            "petal" => Some(Self { id_field: "petal_id", geometry: Some("bounds"), undeletable: &["name"] }),
            _ => None,
        }
    }
}

/// A clocked row that passed the strictest reader.
struct ClockedRow {
    rid: String,
    author: String,
    identity: Map<String, Value>,
    incoming: Vec<FieldVersion>,
    /// Unclocked key columns (floor values the publisher does not re-clock).
    plain: Vec<(String, Value)>,
    max_hlc: u64,
}

/// Whether an inbound static row carries the `_fe_clock` column.
pub(super) fn carries_field_clock(row_bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(row_bytes)
        .ok()
        .is_some_and(|row| row.get(wire::CLOCK_COLUMN).is_some_and(|clock| !clock.is_null()))
}

/// Apply one inbound clocked verse/fractal/petal row: every version is stored, only admissible ones win.
pub(crate) async fn apply_clocked_static_row(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<ReplicatedRowOutcome> {
    if row_bytes.is_empty() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let Some(shape) = TableShape::of(table) else {
        anyhow::bail!("{table} is not a clocked hierarchy table");
    };
    let row = match parse_clocked_row(table, &shape, record_id, row_bytes, author_did) {
        Ok(row) => row,
        Err(reason) => {
            tracing::warn!(verse_id, table, record_id, author = author_did, %reason, "Inbound clocked row DENIED by the strictest reader");
            return Ok(ReplicatedRowOutcome::Denied);
        }
    };
    let mut attempt = 0;
    loop {
        attempt += 1;
        match apply_once(db, verse_id, table, &shape, &row).await {
            Err(error) if attempt < FENCE_ATTEMPTS && is_fence_error(&error) => {
                tracing::debug!(verse_id, table, rid = %row.rid, attempt, "clocked row moved under the merge; re-reading");
            }
            outcome => return outcome,
        }
    }
}

/// Strictest reader: the clock column, identity, keys, HLC bound and per-field value rules.
fn parse_clocked_row(
    table: &str,
    shape: &TableShape,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> Result<ClockedRow, String> {
    if row_bytes.len() > super::scene_publication::wire::MAX_ROW_BYTES {
        return Err("row exceeds the row bound".into());
    }
    let row: Value = match serde_json::from_slice(row_bytes) {
        Ok(row @ Value::Object(_)) => row,
        _ => return Err("row is not a JSON object".into()),
    };
    let incoming = match wire::parse_own(table, &row, author_did) {
        Ok(Some(incoming)) => incoming,
        Ok(None) => return Err("row carries no field clock".into()),
        Err(error) => return Err(format!("{error:#}")),
    };
    let rid = match present(&row, shape.id_field) {
        None => record_id.to_owned(),
        Some(Value::String(rid)) if !rid.is_empty() => rid.clone(),
        Some(_) => return Err(format!("{} is not a string", shape.id_field)),
    };
    let mut identity = Map::new();
    for column in wire::identity_columns(table).unwrap_or(&[]) {
        if let Some(value) = present(&row, column) {
            identity.insert((*column).to_owned(), value.clone());
        }
    }
    identity.insert(shape.id_field.to_owned(), Value::String(rid.clone()));
    let mut max_hlc = 0;
    for version in &incoming {
        // Static keys become SurrealQL column names (MERGE keys and UNSET targets).
        if !is_plain_identifier(&version.field) {
            return Err(format!("key {:?} is not a column name", version.field));
        }
        if let Err(error) = crate::op_log::admit_scene_hlc(version.hlc) {
            return Err(format!("key {:?}: {error:#}", version.field));
        }
        max_hlc = max_hlc.max(version.hlc);
        match (&version.value, version.deleted) {
            (_, true) if shape.undeletable.contains(&version.field.as_str()) => {
                return Err(format!("key {:?} cannot be deleted", version.field));
            }
            (Some(Value::Null), false) => {
                return Err(format!("key {:?} carries null; a removal is a deleted version", version.field));
            }
            (Some(value), false) => {
                if let Some(reason) = static_field_violation(table, &version.field, value) {
                    return Err(format!("key {:?}: {reason}", version.field));
                }
            }
            _ => {}
        }
    }
    // Unclocked columns: candidates for a key this store holds no version of (field_clock/AGENTS.md §floor-fill).
    let mut plain = Vec::new();
    for (field, value) in wire::keys_of(table, &row) {
        if field == wire::CLOCK_COLUMN || incoming.iter().any(|version| version.field == field) {
            continue;
        }
        // Plain columns are the publisher's legacy data, not its write: a bad one is skipped, never fatal.
        let rejected = if !is_plain_identifier(&field) || !wire::is_clock_key(table, &field) {
            Some("not a clock key")
        } else {
            static_field_violation(table, &field, &value)
        };
        if let Some(reason) = rejected {
            tracing::warn!(table, record_id, field = %field, reason, "Inbound clocked row: skipped an invalid plain column");
            continue;
        }
        plain.push((field, value));
    }
    Ok(ClockedRow { rid, author: author_did.to_owned(), identity, incoming, plain, max_hlc })
}

/// `[A-Za-z_][A-Za-z0-9_]*`, the only column names the hierarchy schema defines.
fn is_plain_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Whether `error` is one of this module's fence THROWs (retry, never a failure).
pub(super) fn is_fence_error(error: &anyhow::Error) -> bool {
    format!("{error:#}").contains(FENCE_MESSAGE)
}

/// One row's cached winners from `row_field_clock` (value-less: only field, hlc and author are cached).
pub(super) async fn cached_winners(db: &Db, table: &str, rid: &str) -> anyhow::Result<Vec<FieldVersion>> {
    let response = db
        .query("SELECT field_key, hlc, author FROM row_field_clock WHERE tbl=$tbl AND rid=$rid TIMEOUT 5s")
        .bind(("tbl", table.to_owned()))
        .bind(("rid", rid.to_owned()))
        .await?
        .check();
    let rows: Vec<Value> = match response {
        Ok(mut response) => response.take(0)?,
        // A store whose schema has not defined the clock tables yet holds no winners.
        Err(error) if error.to_string().contains("does not exist") => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    rows.iter()
        .map(|row| -> anyhow::Result<FieldVersion> {
            let (Some(field), Some(hlc), Some(author)) =
                (row["field_key"].as_str(), row["hlc"].as_u64(), row["author"].as_str())
            else {
                anyhow::bail!("stored field clock row is invalid");
            };
            Ok(FieldVersion { field: field.to_owned(), author: author.to_owned(), hlc, value: None, deleted: false })
        })
        .collect()
}

/// The [`CLOCK_FENCE`] statement and its bindings for one row's `cached` winners.
pub(super) fn clock_fence(table: &str, rid: &str, cached: &[FieldVersion]) -> anyhow::Result<SqlFragment> {
    let before = cached
        .iter()
        .map(|winner| -> anyhow::Result<Value> { Ok(json!([winner.field, stored_hlc(winner.hlc)?, winner.author])) })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((
        vec![CLOCK_FENCE.into()],
        vec![
            ("fe_row_tbl".into(), json!(table)),
            ("fe_row_rid".into(), json!(rid)),
            ("fe_winners_before".into(), Value::Array(before)),
        ],
    ))
}

/// Run `statements` with `bindings` as one transaction.
pub(super) async fn commit(db: &Db, statements: Vec<String>, bindings: Vec<(String, Value)>) -> anyhow::Result<()> {
    let script = format!("BEGIN TRANSACTION;\n{};\nCOMMIT TRANSACTION;", statements.join(";\n"));
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    Ok(())
}

/// The inline `<geometry<polygon>>` write of a petal `bounds` value (§geometry-inserts).
pub(super) fn geometry_statement(
    table: &str,
    id_field: &str,
    column: &str,
    rid_binding: &str,
    geometry: &Value,
) -> anyhow::Result<String> {
    let rings = geometry
        .get("coordinates")
        .and_then(render_polygon_rings)
        .ok_or_else(|| anyhow::anyhow!("{table} {column} coordinates are not numeric rings"))?;
    Ok(format!(
        "UPDATE {table} SET {column} = <geometry<polygon>> {{ type: 'Polygon', coordinates: {rings} }} WHERE {id_field} = ${rid_binding}"
    ))
}

/// One read-judge-write pass under fences; a fence THROW is retried by the caller.
async fn apply_once(
    db: &Db,
    verse_id: &str,
    table: &str,
    shape: &TableShape,
    row: &ClockedRow,
) -> anyhow::Result<ReplicatedRowOutcome> {
    let rid = row.rid.as_str();
    let stored = lookup_row(db, table, shape.id_field, rid, "* OMIT id").await?;
    // Identity columns are insert-once; only a missing namespace_id may be filled (admission gated it to a verse Manager).
    let mut patch = Map::new();
    match &stored {
        Some(stored) => {
            for (column, incoming) in &row.identity {
                match present(stored, column) {
                    Some(current) if json_eq(current, incoming) => {}
                    None if column == "namespace_id" => {
                        patch.insert(column.clone(), incoming.clone());
                    }
                    _ => return Ok(deny(verse_id, table, rid, &row.author, &format!("{column} is immutable"))),
                }
            }
        }
        None => {
            let missing = wire::identity_columns(table)
                .unwrap_or(&[])
                .iter()
                .find(|column| **column != "namespace_id" && !row.identity.contains_key(**column));
            if let Some(column) = missing {
                return Ok(deny(verse_id, table, rid, &row.author, &format!("a new row lacks {column}")));
            }
        }
    }
    let scope = match table {
        "verse" => crate::scope::build_scope(verse_id, None, None),
        "fractal" => crate::scope::build_scope(verse_id, Some(rid), None),
        _ => {
            let fractal_id = row
                .identity
                .get("fractal_id")
                .or_else(|| stored.as_ref().and_then(|stored| present(stored, "fractal_id")))
                .and_then(Value::as_str);
            let Some(fractal_id) = fractal_id else {
                return Ok(deny(verse_id, table, rid, &row.author, "petal has no fractal"));
            };
            crate::scope::build_scope(verse_id, Some(fractal_id), Some(rid))
        }
    };

    // Merge: fold the author's versions into the stored set (keep-newer per key and author).
    let stored_versions = store::load_versions(db, table, rid).await?;
    let cached = cached_winners(db, table, rid).await?;
    let versions_before = stored_versions
        .iter()
        .map(|version| -> anyhow::Result<Value> { Ok(json!([version.field, version.author, stored_hlc(version.hlc)?])) })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut versions: BTreeMap<(String, String), FieldVersion> = stored_versions
        .into_iter()
        .map(|version| ((version.field.clone(), version.author.clone()), version))
        .collect();
    let mut written = Vec::new();
    for incoming in &row.incoming {
        let key = (incoming.field.clone(), incoming.author.clone());
        if let Some(kept) = merge::upsert_own(versions.get(&key), incoming.clone()) {
            versions.insert(key, kept.clone());
            written.push(kept);
        }
    }
    // Floor fill (database-2, field_clock/AGENTS.md §floor-fill): plain columns of keys with no clocked version here.
    let floor_candidates: Vec<FieldVersion> = if row.author.is_empty() {
        Vec::new()
    } else {
        row.plain
            .iter()
            .filter(|(field, _)| {
                !versions.values().any(|version| &version.field == field && version.hlc > 0)
                    && !cached.iter().any(|winner| &winner.field == field && winner.hlc > 0)
            })
            .filter_map(|(field, value)| {
                let candidate = FieldVersion {
                    field: field.clone(),
                    author: row.author.clone(),
                    hlc: 0,
                    value: Some(value.clone()),
                    deleted: false,
                };
                merge::upsert_own(versions.get(&(field.clone(), row.author.clone())), candidate)
            })
            .collect()
    };
    if stored.is_some() && written.is_empty() && patch.is_empty() && floor_candidates.is_empty() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let view = match (&stored, table) {
        (None, "verse") => VerseView {
            owner: row.identity.get("created_by").and_then(Value::as_str).unwrap_or_default().to_owned(),
            admitted: crate::role_grants::load_admitted(db, verse_id).await?,
        },
        _ => VerseView::load(db, verse_id).await?,
    };
    let mut claims = store::load_floor_claims(db, table, rid).await?;
    let mut new_claims: Vec<(String, String, u64)> = Vec::new();
    let mut withheld_name = false;
    for candidate in floor_candidates {
        // Admitted only if the author held the key's level at the publish hlc (write-time role).
        if !view.admits_at(table, &scope, &FieldVersion { hlc: row.max_hlc, ..candidate.clone() }) {
            withheld_name |= candidate.field == CREATE_REQUIRED_KEY;
            continue;
        }
        let slot = (candidate.field.clone(), candidate.author.clone());
        claims.insert(slot.clone(), row.max_hlc);
        new_claims.push((slot.0.clone(), slot.1.clone(), row.max_hlc));
        versions.insert(slot, candidate.clone());
        written.push(candidate);
    }
    if stored.is_some() && written.is_empty() && patch.is_empty() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }

    // Judge: the winner of each key is the maximum admissible version (write-time role, QA-0165).
    let by_key = admissible_by_key(&view, table, &scope, &versions, &claims);
    let mut winners: BTreeMap<String, FieldVersion> = by_key
        .iter()
        .filter_map(|(field, admissible)| merge::winner(admissible, |_| true).map(|won| ((*field).to_owned(), won.clone())))
        .collect();
    // A key with no admissible version keeps its cached winner; re-judge (CLOCK-10) owns demotions.
    for winner in &cached {
        winners.entry(winner.field.clone()).or_insert_with(|| winner.clone());
    }
    // A winner written by this apply is changed even at its cached (field, hlc, author): the cache is value-less (§floor-fill).
    let is_cached = |winner: &FieldVersion| {
        !written.contains(winner)
            && cached
                .iter()
                .any(|old| old.field == winner.field && old.hlc == winner.hlc && old.author == winner.author)
    };
    let changed: Vec<&FieldVersion> = winners.values().filter(|winner| !is_cached(*winner)).collect();
    let cache_changed = !changed.is_empty() || winners.len() != cached.len();

    if stored.is_none() && !winners.contains_key(CREATE_REQUIRED_KEY) {
        let offered = withheld_name || versions.keys().any(|(field, _)| field == CREATE_REQUIRED_KEY);
        if offered {
            // The name exists but no admissible author wrote it yet: retried when a role rises.
            tracing::debug!(verse_id, table, rid, author = %row.author, "new clocked row has no admissible name yet; deferred");
            return Ok(ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole { did: row.author.clone() }));
        }
        return Ok(deny(verse_id, table, rid, &row.author, "a new row carries no name"));
    }

    // Write: row, versions and winner cache in ONE fenced transaction.
    let (mut statements, mut bindings) = clock_fence(table, rid, &cached)?;
    statements.insert(0, VERSIONS_FENCE.into());
    statements.insert(
        0,
        format!(
            "IF array::len((SELECT VALUE {id} FROM {table} WHERE {id} = $fe_row_rid)) != $fe_row_count {{ THROW '{FENCE_MESSAGE}'; }}",
            id = shape.id_field
        ),
    );
    bindings.push(("fe_row_count".into(), json!(usize::from(stored.is_some()))));
    bindings.push(("fe_versions_before".into(), Value::Array(versions_before)));
    let mut writes = ColumnWrites { patch, ..ColumnWrites::default() };
    let row_targets: Vec<&FieldVersion> = if stored.is_none() { winners.values().collect() } else { changed.clone() };
    writes.add_winners(shape, &row_targets, &versions);
    let ColumnWrites { mut patch, unset, geometry } = writes;
    let created = stored.is_none();
    let row_changed = created || !patch.is_empty() || !unset.is_empty() || geometry.is_some();
    if created {
        for (column, value) in &row.identity {
            patch.insert(column.clone(), value.clone());
        }
        statements.push(format!("CREATE {table} CONTENT $fe_row_content"));
        bindings.push(("fe_row_content".into(), Value::Object(patch)));
        if let (Some(column), Some(value)) = (shape.geometry, &geometry) {
            statements.push(geometry_statement(table, shape.id_field, column, "fe_row_rid", value)?);
        }
    } else {
        ColumnWrites { patch, unset, geometry }.push_update(table, shape, &mut statements, &mut bindings)?;
    }
    if !written.is_empty() {
        let (versions_statements, versions_bindings) = store::versions_write_sql(table, rid, verse_id, &written)?;
        statements.extend(versions_statements);
        bindings.extend(versions_bindings);
    }
    if !new_claims.is_empty() {
        let (claim_statements, claim_bindings) = store::floor_claims_sql(table, rid, &new_claims)?;
        statements.extend(claim_statements);
        bindings.extend(claim_bindings);
    }
    if cache_changed {
        let winner_refs: Vec<&FieldVersion> = winners.values().collect();
        let (clock_statements, clock_bindings) = store::clock_write_sql(table, rid, &winner_refs)?;
        statements.extend(clock_statements);
        bindings.extend(clock_bindings);
    }
    commit(db, statements, bindings).await?;
    if !written.is_empty() {
        // Order later local writes after the remote clock, only once it is committed.
        if let Ok(clock) = crate::op_log::admit_scene_hlc(row.max_hlc) {
            crate::op_log::observe_scene_hlc(clock);
        }
    }
    Ok(if row_changed {
        ReplicatedRowOutcome::Applied
    } else {
        ReplicatedRowOutcome::NotApplicable
    })
}

/// Admissible versions grouped by key; a filled floor is judged at its claim hlc (field_clock/AGENTS.md §floor-fill).
fn admissible_by_key<'a>(
    view: &VerseView,
    table: &str,
    scope: &str,
    versions: &'a BTreeMap<(String, String), FieldVersion>,
    claims: &BTreeMap<(String, String), u64>,
) -> BTreeMap<&'a str, Vec<FieldVersion>> {
    let mut verdicts: HashMap<(String, u64, bool), bool> = HashMap::new();
    let mut by_key: BTreeMap<&'a str, Vec<FieldVersion>> = BTreeMap::new();
    for (slot, version) in versions {
        let manager = required_level(table, &version.field) == RoleLevel::Manager;
        let claim = claims.get(slot).copied();
        let judged_hlc = if version.hlc == 0 && !version.author.is_empty() { claim.unwrap_or(0) } else { version.hlc };
        let admitted = *verdicts
            .entry((version.author.clone(), judged_hlc, manager))
            .or_insert_with(|| view.admits_claimed(table, scope, version, claim));
        if admitted {
            by_key.entry(version.field.as_str()).or_default().push(version.clone());
        }
    }
    by_key
}

/// Column writes of one row update: MERGE patch, UNSET columns and the geometry value.
#[derive(Default)]
struct ColumnWrites {
    patch: Map<String, Value>,
    unset: Vec<String>,
    geometry: Option<Value>,
}

impl ColumnWrites {
    /// Add each target's stored value (a deleted version unsets its column); values come from `versions`, never the value-less cache.
    fn add_winners(
        &mut self,
        shape: &TableShape,
        targets: &[&FieldVersion],
        versions: &BTreeMap<(String, String), FieldVersion>,
    ) {
        for winner in targets {
            let Some(version) = versions.get(&(winner.field.clone(), winner.author.clone())) else {
                continue;
            };
            let value = version.value.as_ref().filter(|_| !version.deleted);
            match (value, Some(version.field.as_str()) == shape.geometry) {
                (Some(value), true) => self.geometry = Some(value.clone()),
                (Some(value), false) => {
                    self.patch.insert(version.field.clone(), value.clone());
                }
                (None, _) => self.unset.push(version.field.clone()),
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.patch.is_empty() && self.unset.is_empty() && self.geometry.is_none()
    }

    /// Append the UPDATE statements of an existing row keyed by `$fe_row_rid`.
    fn push_update(
        self,
        table: &str,
        shape: &TableShape,
        statements: &mut Vec<String>,
        bindings: &mut Vec<(String, Value)>,
    ) -> anyhow::Result<()> {
        if !self.patch.is_empty() {
            statements.push(format!("UPDATE {table} MERGE $fe_row_patch WHERE {} = $fe_row_rid", shape.id_field));
            bindings.push(("fe_row_patch".into(), Value::Object(self.patch)));
        }
        if !self.unset.is_empty() {
            // Every name passed `is_plain_identifier`.
            statements.push(format!("UPDATE {table} UNSET {} WHERE {} = $fe_row_rid", self.unset.join(", "), shape.id_field));
        }
        if let (Some(column), Some(value)) = (shape.geometry, &self.geometry) {
            statements.push(geometry_statement(table, shape.id_field, column, "fe_row_rid", value)?);
        }
        Ok(())
    }
}

/// Re-judge one stored verse/fractal/petal row from its stored versions alone; see field_clock/AGENTS.md §rejudge.
/// `Applied` when a column changed; winners may be demoted, but a required column never loses its last value.
/// `local_did` judges unstamped local values (field_clock/AGENTS.md §rejudge-pending); `None` keeps them.
pub(crate) async fn rematerialize(
    db: &Db,
    verse_id: &str,
    table: &str,
    rid: &str,
    view: &VerseView,
    local_did: Option<&str>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    let Some(shape) = TableShape::of(table) else {
        anyhow::bail!("{table} is not a clocked hierarchy table");
    };
    let mut attempt = 0;
    loop {
        attempt += 1;
        match rematerialize_once(db, verse_id, table, &shape, rid, view, local_did).await {
            Err(error) if attempt < FENCE_ATTEMPTS && is_fence_error(&error) => {
                tracing::debug!(verse_id, table, rid, attempt, "clocked row moved under the re-judge; re-reading");
            }
            outcome => return outcome,
        }
    }
}

/// Deepest scope of a stored hierarchy row, `None` when it is unknown or sits in another verse.
async fn stored_scope(db: &Db, verse_id: &str, table: &str, rid: &str) -> anyhow::Result<Option<String>> {
    use crate::handlers::crud::{
        resolve_fractal_scope_handler, resolve_petal_scope_handler, resolve_verse_scope_handler,
    };
    let scope = match table {
        "verse" if rid == verse_id => resolve_verse_scope_handler(db, rid).await?,
        "fractal" => resolve_fractal_scope_handler(db, rid).await?,
        "petal" => resolve_petal_scope_handler(db, rid).await?,
        _ => None,
    };
    Ok(scope.filter(|scope| crate::scope::parse_scope(scope).is_ok_and(|parts| parts.verse_id == verse_id)))
}

/// Clock keys whose committed column differs from its cached winner's value: a local write no publish has stamped yet.
fn unstamped_local_keys(
    table: &str,
    row: &Value,
    cached: &[FieldVersion],
    versions: &BTreeMap<(String, String), FieldVersion>,
) -> BTreeSet<String> {
    let local: BTreeMap<String, Value> = wire::keys_of(table, row)
        .into_iter()
        .filter(|(field, _)| wire::is_clock_key(table, field))
        .map(|(field, value)| (field, merge::canonical_value(&value)))
        .collect();
    let mut pending = BTreeSet::new();
    for winner in cached {
        // A cached winner whose version is gone is stale; nothing proves what was published.
        let Some(version) = versions
            .get(&(winner.field.clone(), winner.author.clone()))
            .filter(|version| version.hlc == winner.hlc)
        else {
            continue;
        };
        let published = version.value.as_ref().filter(|_| !version.deleted).map(merge::canonical_value);
        if local.get(&winner.field) != published.as_ref() {
            pending.insert(winner.field.clone());
        }
    }
    pending
}

/// One read-judge-write re-judge pass under the same fences as [`apply_once`].
async fn rematerialize_once(
    db: &Db,
    verse_id: &str,
    table: &str,
    shape: &TableShape,
    rid: &str,
    view: &VerseView,
    local_did: Option<&str>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    let Some(stored_row) = lookup_row(db, table, shape.id_field, rid, "* OMIT id").await? else {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    };
    let Some(scope) = stored_scope(db, verse_id, table, rid).await? else {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    };
    let stored_versions = store::load_versions(db, table, rid).await?;
    if stored_versions.is_empty() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let cached = cached_winners(db, table, rid).await?;
    let versions_before = stored_versions
        .iter()
        .map(|version| -> anyhow::Result<Value> { Ok(json!([version.field, version.author, stored_hlc(version.hlc)?])) })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let versions: BTreeMap<(String, String), FieldVersion> = stored_versions
        .into_iter()
        .map(|version| ((version.field.clone(), version.author.clone()), version))
        .collect();

    // Judge every stored version afresh against the admitted statements (write-time role, QA-0165).
    let claims = store::load_floor_claims(db, table, rid).await?;
    let by_key = admissible_by_key(view, table, &scope, &versions, &claims);
    let mut winners: BTreeMap<String, FieldVersion> = by_key
        .iter()
        .filter_map(|(field, admissible)| merge::winner(admissible, |_| true).map(|won| ((*field).to_owned(), won.clone())))
        .collect();
    // A required column with no admissible version keeps its cached winner, so the row stays valid.
    for winner in &cached {
        if shape.undeletable.contains(&winner.field.as_str()) && !winners.contains_key(&winner.field) {
            tracing::debug!(verse_id, table, rid, field = %winner.field, "required key has no admissible version; keeping its value");
            winners.insert(winner.field.clone(), winner.clone());
        }
    }
    let is_cached = |winner: &FieldVersion| {
        cached
            .iter()
            .any(|old| old.field == winner.field && old.hlc == winner.hlc && old.author == winner.author)
    };
    let changed: Vec<&FieldVersion> = winners.values().filter(|winner| !is_cached(*winner)).collect();
    if changed.is_empty() && winners.len() == cached.len() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    // An unstamped local value keeps its column for the next publish to stamp, unless the local author
    // could not stamp it now (the stamp is its write; field_clock/AGENTS.md §rejudge-pending).
    // Judged at the current statements: hierarchy writes record no (actor, HLC) to judge instead (same section, Residual).
    let pending: BTreeSet<String> = unstamped_local_keys(table, &stored_row, &cached, &versions)
        .into_iter()
        .filter(|field| {
            local_did.is_none_or(|did| {
                view.admits_at(
                    table,
                    &scope,
                    &FieldVersion { field: field.clone(), author: did.to_owned(), hlc: u64::MAX, value: None, deleted: true },
                )
            })
        })
        .collect();
    let changed: Vec<&FieldVersion> = changed.into_iter().filter(|winner| !pending.contains(&winner.field)).collect();
    let mut writes = ColumnWrites::default();
    writes.add_winners(shape, &changed, &versions);
    // A demoted key (no admissible version left) loses its column.
    for old in cached.iter().filter(|old| !winners.contains_key(&old.field) && !pending.contains(&old.field)) {
        if is_plain_identifier(&old.field) && !writes.unset.contains(&old.field) {
            writes.unset.push(old.field.clone());
        }
    }
    let row_changed = !writes.is_empty();

    let (mut statements, mut bindings) = clock_fence(table, rid, &cached)?;
    statements.insert(0, VERSIONS_FENCE.into());
    statements.insert(
        0,
        format!(
            "IF array::len((SELECT VALUE {id} FROM {table} WHERE {id} = $fe_row_rid)) != 1 {{ THROW '{FENCE_MESSAGE}'; }}",
            id = shape.id_field
        ),
    );
    bindings.push(("fe_versions_before".into(), Value::Array(versions_before)));
    writes.push_update(table, shape, &mut statements, &mut bindings)?;
    let winner_refs: Vec<&FieldVersion> = winners.values().collect();
    let (clock_statements, clock_bindings) = store::clock_write_sql(table, rid, &winner_refs)?;
    statements.extend(clock_statements);
    bindings.extend(clock_bindings);
    commit(db, statements, bindings).await?;
    Ok(if row_changed {
        ReplicatedRowOutcome::Applied
    } else {
        ReplicatedRowOutcome::NotApplicable
    })
}

/// Log and return the `Denied` outcome for a structural violation found after parsing.
fn deny(verse_id: &str, table: &str, rid: &str, author: &str, reason: &str) -> ReplicatedRowOutcome {
    tracing::warn!(verse_id, table, rid, author, reason, "Inbound clocked row DENIED — not applied");
    ReplicatedRowOutcome::Denied
}
