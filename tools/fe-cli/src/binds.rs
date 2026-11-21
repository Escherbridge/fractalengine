//! Journal `binds`: name paths of pre-existing ids, recorded the first time an agent uses them (AGENTS.md §replay-binding).

use std::path::Path;

use serde_json::Value;

use crate::commands::hierarchy::{name_index, read_verses};
use crate::context::Context;
use crate::journal::{self, Bind};
use crate::transport::Client;

/// Argument keys that carry hierarchy ids, with their level.
const ID_KEYS: [(&str, &str); 4] = [("verse_id", "verse"), ("fractal_id", "fractal"), ("petal_id", "petal"), ("node_id", "node")];

/// Binds for ids in `args` that this agent neither created nor bound before; created ids are remembered.
/// Names come from the loaded context when it covers the id's chain, else from one hierarchy read. Best effort.
pub async fn collect(client: &Client, context: &Context, args: Option<&Value>, created: &[String], journal_path: &Path, agent: Option<&str>) -> Vec<Bind> {
    let sidecar = journal::known_ids_path(journal_path, agent);
    let mut known = journal::load_known_ids(&sidecar);
    let _ = journal::remember_ids(&sidecar, created);
    known.extend(created.iter().cloned());
    let mut fresh: Vec<(&str, String)> = Vec::new();
    // The context ids come first: replay passes them as `--verse/--fractal/--petal` on every step.
    let from_context = [("verse", context.verse.as_ref()), ("fractal", context.fractal.as_ref()), ("petal", context.petal.as_ref())];
    let from_args = ID_KEYS.map(|(key, level)| (level, args.and_then(|args| args.get(key)).and_then(Value::as_str)));
    for (level, id) in from_context.map(|(level, named)| (level, named.map(|named| named.id.as_str()))).into_iter().chain(from_args) {
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            if !known.contains(id) && !fresh.iter().any(|(_, seen)| seen == id) {
                fresh.push((level, id.to_string()));
            }
        }
    }
    if fresh.is_empty() {
        return Vec::new();
    }
    let mut binds = Vec::new();
    let mut unnamed: Vec<(&str, String)> = Vec::new();
    for (level, id) in fresh {
        match context_path(context, level, &id) {
            Some(path) => binds.push(Bind { id, level: level.to_string(), path }),
            None => unnamed.push((level, id)),
        }
    }
    let mut remembered: Vec<String> = binds.iter().map(|bind| bind.id.clone()).collect();
    if !unnamed.is_empty() {
        // One read covers every remaining id; a failed read records nothing and is retried on the next use.
        if let Ok(read) = read_verses(client, context, None).await {
            let index = name_index(&read.verses);
            for (level, id) in unnamed {
                if let Some(found) = index.iter().find(|named| named.id == id && named.level == level) {
                    binds.push(Bind { id: id.clone(), level: level.to_string(), path: found.path.clone() });
                }
                remembered.push(id);
            }
        }
    }
    let _ = journal::remember_ids(&sidecar, &remembered);
    binds
}

/// The name path of `id` when the context names every level down to it.
fn context_path(context: &Context, level: &str, id: &str) -> Option<Vec<String>> {
    let chain = [context.verse.as_ref(), context.fractal.as_ref(), context.petal.as_ref()];
    let depth = match level {
        "verse" => 1,
        "fractal" => 2,
        "petal" => 3,
        _ => return None,
    };
    let named: Option<Vec<&crate::context::NamedRef>> = chain[..depth].iter().copied().collect();
    let named = named?;
    (named.last()?.id == id && named.iter().all(|entry| !entry.name.is_empty())).then(|| named.iter().map(|entry| entry.name.clone()).collect())
}
