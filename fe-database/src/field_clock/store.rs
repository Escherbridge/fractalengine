//! LOCAL `row_field_version` / `row_field_clock` reads and in-transaction write fragments; see field_clock/AGENTS.md §storage.

use super::merge::canonical_value;
use super::wire::FieldVersion;
use crate::repo::Db;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Statements plus bindings a caller splices into its own `BEGIN TRANSACTION ... COMMIT` script.
pub(crate) type SqlFragment = (Vec<String>, Vec<(String, Value)>);

/// Upsert one version per (tbl, rid, field, author) from `$fe_clock_versions`.
pub(crate) const VERSIONS_WRITE: &str = "FOR $fe_version IN $fe_clock_versions { UPSERT row_field_version SET tbl=$fe_version.tbl, rid=$fe_version.rid, verse_id=$fe_version.verse_id, field_key=$fe_version.field_key, author=$fe_version.author, hlc=$fe_version.hlc, value_json=$fe_version.value_json, deleted=$fe_version.deleted WHERE tbl=$fe_version.tbl AND rid=$fe_version.rid AND field_key=$fe_version.field_key AND author=$fe_version.author; }";
/// SurrealDB 3 can silently skip an `UPSERT ... WHERE` create; prove every version landed.
pub(crate) const VERSIONS_VERIFY: &str = "FOR $fe_version IN $fe_clock_versions { IF array::len((SELECT VALUE hlc FROM row_field_version WHERE tbl=$fe_version.tbl AND rid=$fe_version.rid AND field_key=$fe_version.field_key AND author=$fe_version.author AND hlc=$fe_version.hlc)) != 1 { THROW 'field version was not written'; }; }";
/// Drop cached winners of keys that no longer have one.
pub(crate) const CLOCK_PRUNE: &str = "DELETE row_field_clock WHERE tbl=$fe_clock_tbl AND rid=$fe_clock_rid AND field_key NOTINSIDE $fe_clock_fields";
/// Upsert the winner cache from `$fe_clock_winners`.
pub(crate) const CLOCK_WRITE: &str = "FOR $fe_winner IN $fe_clock_winners { UPSERT row_field_clock SET tbl=$fe_winner.tbl, rid=$fe_winner.rid, field_key=$fe_winner.field_key, hlc=$fe_winner.hlc, author=$fe_winner.author WHERE tbl=$fe_winner.tbl AND rid=$fe_winner.rid AND field_key=$fe_winner.field_key; }";
/// Prove every cached winner landed (same SurrealDB 3 UPSERT caveat).
pub(crate) const CLOCK_VERIFY: &str = "FOR $fe_winner IN $fe_clock_winners { IF array::len((SELECT VALUE hlc FROM row_field_clock WHERE tbl=$fe_winner.tbl AND rid=$fe_winner.rid AND field_key=$fe_winner.field_key AND hlc=$fe_winner.hlc AND author=$fe_winner.author)) != 1 { THROW 'field clock was not written'; }; }";

/// The stored integer form of an HLC (SurrealDB ints are i64).
pub(crate) fn stored_hlc(hlc: u64) -> anyhow::Result<i64> {
    i64::try_from(hlc).map_err(|_| anyhow::anyhow!("field HLC {hlc} exceeds the stored range"))
}

/// One `row_field_version` binding element; `value` (stored as `value_json`) is canonical JSON text and is omitted (NONE) when deleted.
pub(crate) fn version_element(
    table: &str,
    rid: &str,
    verse_id: &str,
    version: &FieldVersion,
) -> anyhow::Result<Value> {
    let mut element = json!({
        "tbl": table,
        "rid": rid,
        "verse_id": verse_id,
        "field_key": version.field,
        "author": version.author,
        "hlc": stored_hlc(version.hlc)?,
        "deleted": version.deleted,
    });
    if let (Some(value), false) = (&version.value, version.deleted) {
        element["value_json"] = Value::String(serde_json::to_string(&canonical_value(value))?);
    }
    Ok(element)
}

/// One `row_field_clock` binding element.
pub(crate) fn winner_element(table: &str, rid: &str, winner: &FieldVersion) -> anyhow::Result<Value> {
    Ok(json!({
        "tbl": table,
        "rid": rid,
        "field_key": winner.field,
        "hlc": stored_hlc(winner.hlc)?,
        "author": winner.author,
    }))
}

/// Write `versions` (already folded through `upsert_own`) for one row, with a verify statement.
pub(crate) fn versions_write_sql(
    table: &str,
    rid: &str,
    verse_id: &str,
    versions: &[FieldVersion],
) -> anyhow::Result<SqlFragment> {
    let elements = versions
        .iter()
        .map(|version| version_element(table, rid, verse_id, version))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((
        vec![VERSIONS_WRITE.into(), VERSIONS_VERIFY.into()],
        vec![("fe_clock_versions".into(), Value::Array(elements))],
    ))
}

/// Replace one row's winner cache with `winners` (one per key), with prune and verify statements.
pub(crate) fn clock_write_sql(
    table: &str,
    rid: &str,
    winners: &[&FieldVersion],
) -> anyhow::Result<SqlFragment> {
    let elements = winners
        .iter()
        .map(|winner| winner_element(table, rid, winner))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let fields: Vec<Value> = winners.iter().map(|winner| json!(winner.field)).collect();
    Ok((
        vec![CLOCK_PRUNE.into(), CLOCK_WRITE.into(), CLOCK_VERIFY.into()],
        vec![
            ("fe_clock_tbl".into(), json!(table)),
            ("fe_clock_rid".into(), json!(rid)),
            ("fe_clock_fields".into(), Value::Array(fields)),
            ("fe_clock_winners".into(), Value::Array(elements)),
        ],
    ))
}

/// Decode one stored `row_field_version` row.
fn version_from_row(row: &Value) -> anyhow::Result<FieldVersion> {
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("stored field version lacks {key}"))
    };
    let deleted = row.get("deleted").and_then(Value::as_bool).unwrap_or(false);
    let value = match row.get("value_json").and_then(Value::as_str) {
        Some(json) if !deleted => Some(serde_json::from_str(json)?),
        _ => None,
    };
    anyhow::ensure!(deleted || value.is_some(), "stored field version has neither value nor deleted");
    Ok(FieldVersion {
        field: text("field_key")?,
        author: text("author")?,
        hlc: row
            .get("hlc")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("stored field version HLC is invalid"))?,
        value,
        deleted,
    })
}

/// Every stored version of one row, ordered by (field, author).
pub(crate) async fn load_versions(db: &Db, tbl: &str, rid: &str) -> anyhow::Result<Vec<FieldVersion>> {
    let mut response = db
        .query("SELECT field_key, author, hlc, value_json, deleted FROM row_field_version WHERE tbl=$tbl AND rid=$rid ORDER BY field_key, author TIMEOUT 5s")
        .bind(("tbl", tbl.to_owned()))
        .bind(("rid", rid.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    rows.iter().map(version_from_row).collect()
}

/// Distinct (tbl, rid) of a verse's rows holding a version by any of `dids`, sorted.
pub(crate) async fn load_rows_by_authors(
    db: &Db,
    verse_id: &str,
    dids: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    if dids.is_empty() {
        return Ok(Vec::new());
    }
    let mut response = db
        .query("SELECT tbl, rid FROM row_field_version WHERE verse_id=$verse_id AND author INSIDE $dids GROUP BY tbl, rid TIMEOUT 10s")
        .bind(("verse_id", verse_id.to_owned()))
        .bind(("dids", dids.to_vec()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    let mut distinct = BTreeSet::new();
    for row in rows {
        let (Some(tbl), Some(rid)) = (row["tbl"].as_str(), row["rid"].as_str()) else {
            anyhow::bail!("stored field version row lacks tbl or rid");
        };
        distinct.insert((tbl.to_owned(), rid.to_owned()));
    }
    Ok(distinct.into_iter().collect())
}

/// Highest positive HLC `did` wrote in the verse: its field versions and the scene clocks it published.
pub(crate) async fn author_frontier(db: &Db, verse_id: &str, did: &str) -> anyhow::Result<Option<u64>> {
    let mut response = db
        .query(
            "SELECT math::max(hlc) AS max_clock FROM row_field_version WHERE verse_id=$verse_id AND author=$did GROUP ALL TIMEOUT 5s;
             SELECT math::max(hlc) AS max_clock FROM node_scene_clock WHERE publisher=$did AND node_id INSIDE (SELECT VALUE node_id FROM node WHERE petal_id INSIDE (SELECT VALUE petal_id FROM petal WHERE fractal_id INSIDE (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$verse_id))) GROUP ALL TIMEOUT 5s;",
        )
        .bind(("verse_id", verse_id.to_owned()))
        .bind(("did", did.to_owned()))
        .await?
        .check()?;
    let mut frontier: Option<u64> = None;
    for index in 0..2usize {
        let rows: Vec<Value> = response.take(index)?;
        let max = match rows.first().and_then(|row| row.get("max_clock")) {
            None | Some(Value::Null) => continue,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("stored HLC is invalid"))?,
        };
        if max > 0 {
            frontier = Some(frontier.map_or(max, |current| current.max(max)));
        }
    }
    Ok(frontier)
}
