//! Outbound clocked hierarchy rows; see field_clock/AGENTS.md §publish.

use super::merge::{canonical_value, upsert_own, version_order};
use super::store::{clock_write_sql, load_versions, versions_write_sql};
use super::wire::{encode_own, keys_of, FieldVersion, CLOCK_COLUMN, MAX_OWN_KEYS};
use crate::repo::Db;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Replicated columns and identity column of a clocked hierarchy table; `None` for any other table.
pub(crate) fn row_select(table: &str) -> Option<(&'static str, &'static str)> {
    match table {
        "verse" => Some((
            "SELECT verse_id, name, created_by, created_at, namespace_id, \
            default_access, ts_mode, ts_replication_factor, ts_bucket_width_ms FROM verse",
            "verse_id",
        )),
        "fractal" => Some((
            "SELECT fractal_id, verse_id, owner_did, name, description, created_at FROM fractal",
            "fractal_id",
        )),
        "petal" => Some((
            "SELECT petal_id, fractal_id, name, node_id, bounds, created_at, description, visibility, tags, \
            hexon_manifest, terrain, visibility_groups, render_distance FROM petal",
            "petal_id",
        )),
        _ => None,
    }
}

/// Stamp the local author's own versions of a row (`restamp` = re-clock changed keys) and publish it.
pub(crate) async fn publish_clocked_row(
    db: &Db,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    verse_id: &str,
    table: &str,
    rid: &str,
    restamp: bool,
) -> anyhow::Result<()> {
    if sender.is_none() {
        return Ok(());
    }
    let (select, identity) = row_select(table)
        .ok_or_else(|| anyhow::anyhow!("table {table} is not a clocked hierarchy table"))?;
    let mut response = db
        .query(format!("{select} WHERE {identity}=$clocked_rid LIMIT 1 TIMEOUT 5s"))
        .bind(("clocked_rid", rid.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("committed {table} row {rid} is missing"))?;
    publish_clocked_value(db, blobs, sender, local_did, verse_id, table, rid, row, restamp).await
}

/// [`publish_clocked_row`] for a caller that already read the committed row through its own guard.
pub(crate) async fn publish_clocked_value(
    db: &Db,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    verse_id: &str,
    table: &str,
    rid: &str,
    row: Value,
    restamp: bool,
) -> anyhow::Result<()> {
    if sender.is_none() {
        return Ok(());
    }
    anyhow::ensure!(!local_did.is_empty(), "cannot clock a row without a local identity");
    let row = crate::repo::strip_json_nulls(row);
    let mut versions = load_versions(db, table, rid).await?;
    if restamp {
        let stamped = stamp_changed_keys(db, local_did, verse_id, table, rid, &row, &versions).await?;
        versions = stamped;
    }
    let mut wire_row = row;
    let has_own = versions
        .iter()
        .any(|version| version.author == local_did && version.hlc > 0);
    if has_own {
        let own_count = versions
            .iter()
            .filter(|version| version.author == local_did && version.hlc > 0)
            .count();
        anyhow::ensure!(own_count <= MAX_OWN_KEYS, "{table}/{rid} carries more than {MAX_OWN_KEYS} clocked keys");
        if let Some(object) = wire_row.as_object_mut() {
            object.insert(CLOCK_COLUMN.to_owned(), encode_own(local_did, &versions));
        }
    }
    let bytes = serde_json::to_vec(&wire_row)?;
    if table == "petal" {
        crate::replicate_row_with_petal(sender, blobs, verse_id, table, rid, &bytes, Some(rid.to_owned()));
    } else {
        crate::replicate_row(sender, blobs, verse_id, table, rid, &bytes);
    }
    Ok(())
}

/// Stamp every key of `row` that differs from its stored winner as a local own version; returns all versions.
async fn stamp_changed_keys(
    db: &Db,
    local_did: &str,
    verse_id: &str,
    table: &str,
    rid: &str,
    row: &Value,
    existing: &[FieldVersion],
) -> anyhow::Result<Vec<FieldVersion>> {
    let mut by_field: BTreeMap<&str, Vec<&FieldVersion>> = BTreeMap::new();
    for version in existing {
        by_field.entry(version.field.as_str()).or_default().push(version);
    }
    let current_winner = |field: &str| stored_winner(by_field.get(field).map(Vec::as_slice).unwrap_or(&[]));

    let keys = keys_of(table, row);
    let present: BTreeSet<&str> = keys.iter().map(|(field, _)| field.as_str()).collect();
    let mut changed: Vec<(String, Option<Value>)> = Vec::new();
    for (field, value) in &keys {
        let unchanged = current_winner(field).is_some_and(|stored| {
            !stored.deleted
                && stored
                    .value
                    .as_ref()
                    .is_some_and(|stored_value| canonical_value(stored_value) == canonical_value(value))
        });
        if !unchanged {
            changed.push((field.clone(), Some(value.clone())));
        }
    }
    for field in by_field.keys() {
        let live = current_winner(field).is_some_and(|stored| !stored.deleted);
        if live && !present.contains(field) {
            changed.push(((*field).to_owned(), None));
        }
    }
    if changed.is_empty() {
        return Ok(existing.to_vec());
    }

    // One HLC for the whole publish: causally after everything admitted, since admission advances it.
    let hlc = crate::op_log::next_hlc_timestamp().0;
    let mut merged: Vec<FieldVersion> = existing.to_vec();
    let mut written: Vec<FieldVersion> = Vec::with_capacity(changed.len());
    for (field, value) in changed {
        let incoming = FieldVersion {
            field: field.clone(),
            author: local_did.to_owned(),
            hlc,
            deleted: value.is_none(),
            value,
        };
        let stored_own = merged
            .iter()
            .find(|version| version.field == field && version.author == local_did);
        if let Some(kept) = upsert_own(stored_own, incoming) {
            merged.retain(|version| !(version.field == field && version.author == local_did));
            merged.push(kept.clone());
            written.push(kept);
        }
    }
    if written.is_empty() {
        return Ok(existing.to_vec());
    }

    let cached = load_cached_winners(db, table, rid).await?;
    let written_fields: BTreeSet<&str> = written.iter().map(|version| version.field.as_str()).collect();
    let mut winners: Vec<&FieldVersion> = written.iter().collect();
    for (field, (author, cached_hlc)) in &cached {
        if written_fields.contains(field.as_str()) {
            continue;
        }
        if let Some(version) = merged
            .iter()
            .find(|version| &version.field == field && &version.author == author && version.hlc == *cached_hlc)
        {
            winners.push(version);
        }
    }

    let (mut statements, mut bindings) = versions_write_sql(table, rid, verse_id, &written)?;
    let (clock_statements, clock_bindings) = clock_write_sql(table, rid, &winners)?;
    statements.extend(clock_statements);
    bindings.extend(clock_bindings);
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        statements.join(";\n")
    ));
    for binding in bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    Ok(merged)
}

/// The stored winner cache of one row as `field -> (author, hlc)`.
async fn load_cached_winners(
    db: &Db,
    table: &str,
    rid: &str,
) -> anyhow::Result<BTreeMap<String, (String, u64)>> {
    let mut response = db
        .query("SELECT field_key, author, hlc FROM row_field_clock WHERE tbl=$tbl AND rid=$rid TIMEOUT 5s")
        .bind(("tbl", table.to_owned()))
        .bind(("rid", rid.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    let mut cached = BTreeMap::new();
    for row in rows {
        let (Some(field), Some(author), Some(hlc)) = (
            row["field_key"].as_str(),
            row["author"].as_str(),
            row["hlc"].as_u64(),
        ) else {
            anyhow::bail!("stored field clock row is malformed");
        };
        cached.insert(field.to_owned(), (author.to_owned(), hlc));
    }
    Ok(cached)
}

/// Greatest stored version of one key under the merge order (every stored author counts).
fn stored_winner<'a>(group: &[&'a FieldVersion]) -> Option<&'a FieldVersion> {
    group.iter().copied().max_by(|left, right| version_order(left, right))
}
