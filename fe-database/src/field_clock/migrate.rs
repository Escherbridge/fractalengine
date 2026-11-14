//! Idempotent legacy-floor seeding of pre-clock hierarchy rows; see field_clock/AGENTS.md §legacy-migration.

use super::store::{
    stored_hlc, version_element, winner_element, CLOCK_VERIFY, CLOCK_WRITE, VERSIONS_VERIFY,
    VERSIONS_WRITE,
};
use super::wire::{keys_of, FieldVersion};
use crate::repo::Db;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Rows read per page.
const BATCH_ROWS: usize = 1024;
/// Binding bytes per seeding transaction before it is flushed.
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;
/// Clocked tables and their id columns, parents first.
const TABLES: [(&str, &str); 4] = [
    ("verse", "verse_id"),
    ("fractal", "fractal_id"),
    ("petal", "petal_id"),
    ("node", "node_id"),
];
/// Refuse a chunk if a clocked write landed on one of its rows since the page was read.
const FLOOR_RACE_GUARD: &str = "IF array::len((SELECT VALUE rid FROM row_field_clock WHERE tbl=$fe_floor_tbl AND rid INSIDE $fe_floor_rids LIMIT 1)) > 0 { THROW 'legacy floor raced a clocked write'; }";

/// Seed every verse/fractal/petal/node row lacking `row_field_clock` entries; returns the rows seeded.
pub async fn seed_legacy_floor(db: &Db) -> anyhow::Result<usize> {
    let parents = ParentIndex::load(db).await?;
    let mut seeded = 0;
    let mut orphans = 0usize;
    for (table, id_field) in TABLES {
        let clocked = clocked_rids(db, table).await?;
        let mut after = String::new();
        loop {
            let rows = read_page(db, table, id_field, &after).await?;
            let Some(last) = rows.last().and_then(|row| row[id_field].as_str()) else {
                break;
            };
            after = last.to_owned();
            let pending: Vec<(&str, &Value)> = rows
                .iter()
                .filter_map(|row| row[id_field].as_str().map(|rid| (rid, row)))
                .filter(|(rid, _)| !clocked.contains(*rid))
                .collect();
            let scene_clocks = if table == "node" {
                read_scene_clocks(db, pending.iter().map(|(rid, _)| *rid)).await?
            } else {
                HashMap::new()
            };
            let mut chunk = FloorChunk::new(table);
            for (rid, row) in pending {
                let Some(verse_id) = parents.verse_of(table, row) else {
                    orphans += 1;
                    continue;
                };
                let seed_clock = scene_clocks.get(rid);
                let versions: Vec<FieldVersion> = keys_of(table, row)
                    .into_iter()
                    .map(|(field, value)| match seed_clock {
                        Some((hlc, publisher)) => FieldVersion {
                            field,
                            author: publisher.clone(),
                            hlc: *hlc,
                            value: Some(value),
                            deleted: false,
                        },
                        None => FieldVersion::legacy_floor(field, value),
                    })
                    .collect();
                if versions.is_empty() {
                    continue;
                }
                chunk.push(rid, verse_id, &versions)?;
                seeded += 1;
                if chunk.bytes >= MAX_CHUNK_BYTES {
                    chunk.flush(db).await?;
                }
            }
            chunk.flush(db).await?;
            if rows.len() < BATCH_ROWS {
                break;
            }
        }
    }
    if orphans > 0 {
        tracing::warn!(orphans, "legacy floor skipped rows with no resolvable verse");
    }
    Ok(seeded)
}

/// Parent links needed to resolve a row's verse.
struct ParentIndex {
    fractal_verse: HashMap<String, String>,
    petal_fractal: HashMap<String, String>,
}

impl ParentIndex {
    async fn load(db: &Db) -> anyhow::Result<Self> {
        let mut response = db
            .query(
                "SELECT fractal_id AS child, verse_id AS parent FROM fractal TIMEOUT 30s;
                 SELECT petal_id AS child, fractal_id AS parent FROM petal TIMEOUT 30s;",
            )
            .await?
            .check()?;
        let fractal_verse = parent_map(response.take(0usize)?);
        let petal_fractal = parent_map(response.take(1usize)?);
        Ok(Self { fractal_verse, petal_fractal })
    }

    /// The verse a row belongs to; `None` for an orphan.
    fn verse_of<'a>(&'a self, table: &str, row: &'a Value) -> Option<&'a str> {
        let fractal = match table {
            "verse" | "fractal" => return row["verse_id"].as_str(),
            "petal" => row["fractal_id"].as_str()?,
            "node" => self.petal_fractal.get(row["petal_id"].as_str()?)?.as_str(),
            _ => return None,
        };
        self.fractal_verse.get(fractal).map(String::as_str)
    }
}

/// `child -> parent` from `SELECT .. AS child, .. AS parent` rows.
fn parent_map(rows: Vec<Value>) -> HashMap<String, String> {
    rows.into_iter()
        .filter_map(|row| Some((row["child"].as_str()?.to_owned(), row["parent"].as_str()?.to_owned())))
        .collect()
}

/// Record ids of `table` that already have a cached winner.
async fn clocked_rids(db: &Db, table: &str) -> anyhow::Result<HashSet<String>> {
    let mut response = db
        .query("SELECT VALUE rid FROM row_field_clock WHERE tbl=$tbl TIMEOUT 30s")
        .bind(("tbl", table.to_owned()))
        .await?
        .check()?;
    let rids: Vec<String> = response.take(0)?;
    Ok(rids.into_iter().collect())
}

/// One page of rows ordered by id, strictly after `after`.
async fn read_page(db: &Db, table: &str, id_field: &str, after: &str) -> anyhow::Result<Vec<Value>> {
    // `table` and `id_field` come from the TABLES constant, never from input.
    let statement = format!(
        "SELECT * OMIT id FROM {table} WHERE {id_field} > $after ORDER BY {id_field} LIMIT {BATCH_ROWS} TIMEOUT 30s"
    );
    let mut response = db
        .query(statement)
        .bind(("after", after.to_owned()))
        .await?
        .check()?;
    Ok(response.take(0)?)
}

/// `node_scene_clock` (hlc, publisher) of the given nodes.
async fn read_scene_clocks<'a>(
    db: &Db,
    node_ids: impl Iterator<Item = &'a str>,
) -> anyhow::Result<HashMap<String, (u64, String)>> {
    let ids: Vec<String> = node_ids.map(str::to_owned).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut response = db
        .query("SELECT node_id, hlc, publisher FROM node_scene_clock WHERE node_id INSIDE $ids TIMEOUT 30s")
        .bind(("ids", ids))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    let mut clocks = HashMap::with_capacity(rows.len());
    for row in rows {
        let (Some(node_id), Some(hlc), Some(publisher)) = (
            row["node_id"].as_str(),
            row["hlc"].as_u64().filter(|hlc| *hlc > 0),
            row["publisher"].as_str().filter(|publisher| !publisher.is_empty()),
        ) else {
            anyhow::bail!("stored scene clock is invalid");
        };
        clocks.insert(node_id.to_owned(), (hlc, publisher.to_owned()));
    }
    Ok(clocks)
}

/// Accumulated versions and winners of one table, written in one guarded transaction.
struct FloorChunk {
    table: &'static str,
    rids: Vec<Value>,
    versions: Vec<Value>,
    winners: Vec<Value>,
    bytes: usize,
}

impl FloorChunk {
    fn new(table: &'static str) -> Self {
        Self { table, rids: Vec::new(), versions: Vec::new(), winners: Vec::new(), bytes: 0 }
    }

    /// Add one row; each seeded version is the sole, and therefore winning, version of its key.
    fn push(&mut self, rid: &str, verse_id: &str, versions: &[FieldVersion]) -> anyhow::Result<()> {
        for version in versions {
            stored_hlc(version.hlc)?;
            let element = version_element(self.table, rid, verse_id, version)?;
            self.bytes += serde_json::to_vec(&element)?.len();
            self.versions.push(element);
            self.winners.push(winner_element(self.table, rid, version)?);
        }
        self.rids.push(json!(rid));
        Ok(())
    }

    /// Commit the chunk (no-op when empty) and reset it.
    async fn flush(&mut self, db: &Db) -> anyhow::Result<()> {
        if self.rids.is_empty() {
            return Ok(());
        }
        let script = format!(
            "BEGIN TRANSACTION; {FLOOR_RACE_GUARD};\n{VERSIONS_WRITE};\n{VERSIONS_VERIFY};\n{CLOCK_WRITE};\n{CLOCK_VERIFY};\nCOMMIT TRANSACTION;"
        );
        db.query(script)
            .bind(("fe_floor_tbl", self.table.to_owned()))
            .bind(("fe_floor_rids", Value::Array(std::mem::take(&mut self.rids))))
            .bind(("fe_clock_versions", Value::Array(std::mem::take(&mut self.versions))))
            .bind(("fe_clock_winners", Value::Array(std::mem::take(&mut self.winners))))
            .await?
            .check()?;
        self.bytes = 0;
        Ok(())
    }
}
