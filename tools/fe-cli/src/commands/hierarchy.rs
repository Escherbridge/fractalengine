//! Hierarchy reads (throttled, one `get_hierarchy` per verse token) and name -> id resolution (S3).

use std::time::Duration;

use serde_json::{json, Value};

use super::{from_cli_error, now_rfc3339, rfc3339_age_millis};
use crate::context::Context;
use crate::instance::read_tokens;
use crate::transport::{Client, Outcome};

/// Minimum spacing of hierarchy reads: every read rebuilds the GUI tree (QA-0125).
pub const HIERARCHY_MIN_INTERVAL_MS: i64 = 2000;

/// Hierarchy level of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Verse,
    Fractal,
    Petal,
}

/// One flattened hierarchy entry with its ancestor ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub level: Level,
    pub id: String,
    pub name: String,
    pub verse_id: Option<String>,
    pub fractal_id: Option<String>,
    /// Number of direct children.
    pub children: usize,
}

/// Outcome of resolving an id or name.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Found(Entry),
    Ambiguous(Vec<Entry>),
    Unknown,
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn items<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

/// Flattens `get_hierarchy` verse objects into entries.
pub fn flatten(verses: &[Value]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for verse in verses {
        let verse_id = text(verse, "id");
        entries.push(Entry {
            level: Level::Verse,
            id: verse_id.clone(),
            name: text(verse, "name"),
            verse_id: None,
            fractal_id: None,
            children: items(verse, "fractals").len(),
        });
        for fractal in items(verse, "fractals") {
            let fractal_id = text(fractal, "id");
            entries.push(Entry {
                level: Level::Fractal,
                id: fractal_id.clone(),
                name: text(fractal, "name"),
                verse_id: Some(verse_id.clone()),
                fractal_id: None,
                children: items(fractal, "petals").len(),
            });
            for petal in items(fractal, "petals") {
                entries.push(Entry {
                    level: Level::Petal,
                    id: text(petal, "id"),
                    name: text(petal, "name"),
                    verse_id: Some(verse_id.clone()),
                    fractal_id: Some(fractal_id.clone()),
                    children: items(petal, "nodes").len(),
                });
            }
        }
    }
    entries
}

/// Resolves an id (passthrough) or a name at one level, optionally confined to a verse.
pub fn resolve(entries: &[Entry], level: Level, query: &str, within_verse: Option<&str>) -> Resolution {
    let candidates: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.level == level)
        .filter(|entry| within_verse.is_none_or(|verse| level == Level::Verse && entry.id == verse || entry.verse_id.as_deref() == Some(verse)))
        .collect();
    if let Some(by_id) = candidates.iter().find(|entry| entry.id == query) {
        return Resolution::Found((*by_id).clone());
    }
    let mut named: Vec<&Entry> = candidates.iter().copied().filter(|entry| entry.name == query).collect();
    if named.is_empty() {
        named = candidates.iter().copied().filter(|entry| entry.name.eq_ignore_ascii_case(query)).collect();
    }
    match named.len() {
        0 => Resolution::Unknown,
        1 => Resolution::Found(named[0].clone()),
        _ => Resolution::Ambiguous(named.into_iter().cloned().collect()),
    }
}

/// The node objects of one petal, from verse objects.
pub fn petal_nodes(verses: &[Value], petal_id: &str) -> Vec<Value> {
    verses
        .iter()
        .flat_map(|verse| items(verse, "fractals"))
        .flat_map(|fractal| items(fractal, "petals"))
        .filter(|petal| text(petal, "id") == petal_id)
        .flat_map(|petal| items(petal, "nodes").to_vec())
        .collect()
}

/// A hierarchy read: verse objects, per-token failures, and the context stamped with the read time.
pub struct HierarchyRead {
    pub verses: Vec<Value>,
    pub errors: Vec<Value>,
    pub context: Context,
}

/// Reads `get_hierarchy` once per verse token (or only `only_verse`), waiting out the 2 s spacing.
pub async fn read_verses(client: &Client, context: &Context, only_verse: Option<&str>) -> Result<HierarchyRead, Outcome> {
    let tokens = read_tokens(&client.instance).map_err(from_cli_error)?;
    let mut verse_ids: Vec<Option<String>> = tokens
        .tokens
        .keys()
        .filter(|verse_id| only_verse.is_none_or(|only| only == verse_id.as_str()))
        .map(|verse_id| Some(verse_id.clone()))
        .collect();
    if verse_ids.is_empty() {
        verse_ids.push(only_verse.map(str::to_string));
    }
    if let Some(age) = context.hierarchy_read_at.as_deref().and_then(rfc3339_age_millis) {
        if (0..HIERARCHY_MIN_INTERVAL_MS).contains(&age) {
            tokio::time::sleep(Duration::from_millis((HIERARCHY_MIN_INTERVAL_MS - age) as u64)).await;
        }
    }
    let mut verses: Vec<Value> = Vec::new();
    let mut errors = Vec::new();
    let mut first_failure: Option<Outcome> = None;
    let mut succeeded = false;
    for verse_id in verse_ids {
        let outcome = client.call_tool("get_hierarchy", json!({}), verse_id.as_deref()).await;
        if outcome.exit != crate::envelope::ExitCode::Ok {
            errors.push(json!({"verse_id": verse_id, "code": outcome.code, "message": outcome.message}));
            first_failure.get_or_insert(outcome);
            continue;
        }
        succeeded = true;
        let listed = match outcome.data {
            Some(Value::Array(listed)) => listed,
            Some(Value::Object(mut wrapper)) => match wrapper.remove("data").or_else(|| wrapper.remove("verses")) {
                Some(Value::Array(listed)) => listed,
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        for verse in listed {
            let id = text(&verse, "id");
            if !verses.iter().any(|known| text(known, "id") == id) {
                verses.push(verse);
            }
        }
    }
    if !succeeded {
        if let Some(failure) = first_failure {
            return Err(failure);
        }
    }
    let mut stamped = context.clone();
    stamped.hierarchy_read_at = Some(now_rfc3339());
    Ok(HierarchyRead { verses, errors, context: stamped })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: Level, id: &str, name: &str, verse: Option<&str>) -> Entry {
        Entry { level, id: id.into(), name: name.into(), verse_id: verse.map(str::to_string), fractal_id: None, children: 0 }
    }

    #[test]
    fn name_to_id_resolution() {
        let entries = vec![
            entry(Level::Verse, "V1", "Campus", None),
            entry(Level::Verse, "V2", "Harbor", None),
            entry(Level::Fractal, "F1", "North", Some("V1")),
            entry(Level::Fractal, "F2", "North", Some("V2")),
            entry(Level::Fractal, "F3", "Annex", Some("V2")),
        ];
        let found = |level, query, within| match resolve(&entries, level, query, within) {
            Resolution::Found(entry) => Some(entry.id),
            _ => None,
        };
        assert_eq!(found(Level::Verse, "Campus", None).as_deref(), Some("V1"), "unique name");
        assert_eq!(found(Level::Verse, "harbor", None).as_deref(), Some("V2"), "case-insensitive fallback");
        assert_eq!(found(Level::Fractal, "F3", None).as_deref(), Some("F3"), "id passthrough");
        assert_eq!(found(Level::Fractal, "North", Some("V2")).as_deref(), Some("F2"), "name confined to a verse");
        assert!(matches!(resolve(&entries, Level::Fractal, "North", None), Resolution::Ambiguous(found) if found.len() == 2));
        assert_eq!(resolve(&entries, Level::Verse, "Nowhere", None), Resolution::Unknown);
        assert_eq!(resolve(&entries, Level::Verse, "F1", None), Resolution::Unknown, "id of another level");
    }
}
