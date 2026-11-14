//! `_fe_clock` wire column and the row <-> clock-key split; see field_clock/AGENTS.md §wire, §keys.

use crate::handlers::scene_publication::wire::{SceneClock, MARKER as SCENE_MARKER};
use serde_json::{json, Map, Value};

/// Top-level column carrying the entry author's own per-key versions.
pub(crate) const CLOCK_COLUMN: &str = "_fe_clock";
/// Most keys one `own` map may carry (spec §3.11).
pub(crate) const MAX_OWN_KEYS: usize = 512;
/// Longest accepted clock key, in bytes.
const MAX_KEY_BYTES: usize = 1024;
/// Prefix of a per-property node key (`properties/<k>`).
pub(crate) const PROPERTY_PREFIX: &str = "properties/";
/// The single node key folding the whole transform.
pub(crate) const TRANSFORM_KEY: &str = "transform";
/// Node columns folded into [`TRANSFORM_KEY`] as one object value.
pub(crate) const NODE_TRANSFORM_COLUMNS: [&str; 5] =
    ["position", "elevation", "rotation", "scale", "edit_seq"];

/// One author's latest version of one key of one row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldVersion {
    pub(crate) field: String,
    pub(crate) author: String,
    pub(crate) hlc: u64,
    pub(crate) value: Option<Value>,
    pub(crate) deleted: bool,
}

impl FieldVersion {
    /// A legacy-floor version (author `''`, hlc 0) seeded by the migration.
    pub(crate) fn legacy_floor(field: String, value: Value) -> Self {
        Self { field, author: String::new(), hlc: 0, value: Some(value), deleted: false }
    }

    /// True for the migration's legacy floor (spec §3.5 rule 2).
    pub(crate) fn is_legacy_floor(&self) -> bool {
        self.author.is_empty() && self.hlc == 0
    }
}

/// Insert-once identity columns of a clocked table; `None` for a table that is not clocked.
pub(crate) fn identity_columns(table: &str) -> Option<&'static [&'static str]> {
    match table {
        "verse" => Some(&["verse_id", "created_by", "created_at", "namespace_id"]),
        "fractal" => Some(&["fractal_id", "verse_id", "owner_did", "created_at"]),
        "petal" => Some(&["petal_id", "fractal_id", "node_id", "created_at"]),
        "node" => Some(&["node_id", "petal_id", "created_at"]),
        _ => None,
    }
}

/// Columns that are never clock keys on any table (record id and wire markers).
fn is_reserved_column(column: &str) -> bool {
    column == "id" || column == CLOCK_COLUMN || column == SCENE_MARKER
}

/// Whether `key` may appear in a `table` row's `own` map.
pub(crate) fn is_clock_key(table: &str, key: &str) -> bool {
    let Some(identity) = identity_columns(table) else {
        return false;
    };
    if key.is_empty() || key.len() > MAX_KEY_BYTES || is_reserved_column(key) {
        return false;
    }
    if let Some(property) = key.strip_prefix(PROPERTY_PREFIX) {
        return table == "node" && !property.is_empty();
    }
    if identity.contains(&key) || key == "properties" || key.contains('/') {
        return false;
    }
    !(table == "node" && NODE_TRANSFORM_COLUMNS.contains(&key))
}

/// Parse a row's `_fe_clock` column; `None` when absent, `Err` on any strictest-reader violation.
pub(crate) fn parse_own(
    table: &str,
    row: &Value,
    entry_author: &str,
) -> anyhow::Result<Option<Vec<FieldVersion>>> {
    let Some(column) = row.get(CLOCK_COLUMN).filter(|column| !column.is_null()) else {
        return Ok(None);
    };
    anyhow::ensure!(identity_columns(table).is_some(), "table {table} is not clocked");
    let clock = column
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("field clock is not an object"))?;
    anyhow::ensure!(
        clock.len() == 3 && clock.get("v") == Some(&json!(1)),
        "unsupported field clock version"
    );
    anyhow::ensure!(
        !entry_author.is_empty() && clock.get("author").and_then(Value::as_str) == Some(entry_author),
        "field clock author is not the entry author"
    );
    let own = clock
        .get("own")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("field clock own map is missing"))?;
    anyhow::ensure!(own.len() <= MAX_OWN_KEYS, "field clock carries more than {MAX_OWN_KEYS} keys");
    let mut versions = Vec::with_capacity(own.len());
    for (field, entry) in own {
        anyhow::ensure!(is_clock_key(table, field), "field clock key {field:?} is not clockable on {table}");
        let entry = entry
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("field clock entry {field:?} is not an object"))?;
        // Same canonical positive-decimal rule as the scene snapshot marker.
        let hlc = SceneClock::from_fields(
            entry.get("hlc").unwrap_or(&Value::Null),
            &Value::String(entry_author.to_owned()),
        )?
        .hlc;
        let deleted = match entry.get("deleted") {
            None => false,
            Some(Value::Bool(true)) => true,
            Some(_) => anyhow::bail!("field clock entry {field:?} has a malformed deleted flag"),
        };
        let value = entry.get("value").cloned();
        anyhow::ensure!(
            deleted != value.is_some() && entry.len() == 2,
            "field clock entry {field:?} must carry exactly one of value or deleted"
        );
        versions.push(FieldVersion {
            field: field.clone(),
            author: entry_author.to_owned(),
            hlc,
            value,
            deleted,
        });
    }
    Ok(Some(versions))
}

/// Encode `author`'s own versions as the `_fe_clock` column; other authors' and floor versions are skipped.
pub(crate) fn encode_own(author: &str, versions: &[FieldVersion]) -> Value {
    let mut own = Map::new();
    for version in versions.iter().filter(|version| version.author == author && version.hlc > 0) {
        let entry = match (&version.value, version.deleted) {
            (Some(value), false) => json!({"hlc": version.hlc.to_string(), "value": value}),
            _ => json!({"hlc": version.hlc.to_string(), "deleted": true}),
        };
        own.insert(version.field.clone(), entry);
    }
    json!({"v": 1, "author": author, "own": own})
}

/// Split a row into its clock keys, sorted by key; null columns are absent and yield no key.
pub(crate) fn keys_of(table: &str, row: &Value) -> Vec<(String, Value)> {
    let (Some(identity), Some(object)) = (identity_columns(table), row.as_object()) else {
        return Vec::new();
    };
    let mut keys = Vec::with_capacity(object.len());
    let mut transform = Map::new();
    for (column, value) in object {
        if value.is_null() || is_reserved_column(column) || identity.contains(&column.as_str()) {
            continue;
        }
        if table == "node" && NODE_TRANSFORM_COLUMNS.contains(&column.as_str()) {
            transform.insert(column.clone(), value.clone());
        } else if table == "node" && column == "properties" {
            if let Some(properties) = value.as_object() {
                keys.extend(
                    properties
                        .iter()
                        .map(|(name, value)| (format!("{PROPERTY_PREFIX}{name}"), value.clone())),
                );
            }
        } else {
            keys.push((column.clone(), value.clone()));
        }
    }
    if !transform.is_empty() {
        keys.push((TRANSFORM_KEY.to_owned(), Value::Object(transform)));
    }
    keys.sort_by(|left, right| left.0.cmp(&right.0));
    keys
}

/// Inverse of [`keys_of`]: overlay each winner on `base_row` (a deleted winner removes its column or property).
pub(crate) fn apply_keys<'a>(
    table: &str,
    base_row: &Value,
    winners: impl IntoIterator<Item = &'a FieldVersion>,
) -> Value {
    let mut row = base_row.as_object().cloned().unwrap_or_default();
    for version in winners {
        if !is_clock_key(table, &version.field) {
            continue;
        }
        let value = version.value.as_ref().filter(|_| !version.deleted);
        if let Some(property) = version.field.strip_prefix(PROPERTY_PREFIX) {
            apply_property(&mut row, property, value);
        } else if table == "node" && version.field == TRANSFORM_KEY {
            for column in NODE_TRANSFORM_COLUMNS {
                match value.and_then(|transform| transform.get(column)) {
                    Some(component) => row.insert(column.to_owned(), component.clone()),
                    None => row.remove(column),
                };
            }
        } else {
            match value {
                Some(value) => row.insert(version.field.clone(), value.clone()),
                None => row.remove(&version.field),
            };
        }
    }
    Value::Object(row)
}

/// Set or remove one node property, creating the `properties` object only for a set.
fn apply_property(row: &mut Map<String, Value>, property: &str, value: Option<&Value>) {
    match value {
        Some(value) => {
            let properties = row
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()));
            if !properties.is_object() {
                *properties = Value::Object(Map::new());
            }
            if let Some(properties) = properties.as_object_mut() {
                properties.insert(property.to_owned(), value.clone());
            }
        }
        None => {
            if let Some(properties) = row.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove(property);
            }
        }
    }
}
