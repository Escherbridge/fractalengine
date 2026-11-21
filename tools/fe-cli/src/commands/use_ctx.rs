//! `fe use` / `fe ctx` (S3; context only, never moves the GUI view).

use serde_json::{json, Value};

use super::hierarchy::{flatten, read_verses, resolve, Entry, Level, Resolution};
use super::{dry_run_outcome, effective_id, make_outcome, ok_outcome, result, usage_outcome, verb_label, CommandRequest, CommandResult, Plan, Scope};
use crate::context::{Context, NamedRef};
use crate::envelope::ExitCode;
use crate::transport::Client;

/// Runs `use instance|verse|fractal|petal` or `ctx [clear]`.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe use instance|verse|fractal|petal <id|name>"));
    };
    match (verb.noun, verb.verb) {
        ("ctx", "") => result(request, context, ok_outcome(json!({ "context": context }))),
        ("ctx", "clear") => {
            let cleared = Context { v: 1, instance: context.instance.clone(), ..Context::default() };
            finish(request, cleared)
        }
        ("use", "instance") => match target(request) {
            Some(name) => {
                let selected = Context { v: 1, instance: Some(name), ..Context::default() };
                finish(request, selected)
            }
            None => result(request, context, usage_outcome("invalid-args", "usage: fe use instance <name>")),
        },
        ("use", "verse") => use_level(request, client, context, Level::Verse).await,
        ("use", "fractal") => use_level(request, client, context, Level::Fractal).await,
        ("use", "petal") => use_level(request, client, context, Level::Petal).await,
        _ => result(request, context, usage_outcome("unknown-verb", "usage: fe use instance|verse|fractal|petal <id|name>, fe ctx [clear]")),
    }
}

fn target(request: &CommandRequest) -> Option<String> {
    let named = request.args.get("target").and_then(Value::as_str).map(str::to_string);
    named.or_else(|| request.positionals.first().cloned()).filter(|text| !text.is_empty())
}

/// Reports the new context and asks the spine to persist it.
fn finish(request: &CommandRequest, new_context: Context) -> CommandResult {
    let data = json!({ "context": new_context });
    let mut finished = result(request, &new_context, ok_outcome(data));
    finished.context_after = Some(new_context);
    finished
}

fn named(entries: &[Entry], level: Level, id: Option<&str>) -> Option<NamedRef> {
    let id = id?;
    entries.iter().find(|entry| entry.level == level && entry.id == id).map(|entry| NamedRef { id: entry.id.clone(), name: entry.name.clone() })
}

/// Resolves an id or name against one hierarchy read and stores it (with its ancestors) in the context.
async fn use_level(request: &CommandRequest, client: &Client, context: &Context, level: Level) -> CommandResult {
    let Some(query) = target(request) else {
        return result(request, context, usage_outcome("invalid-args", "usage: fe use verse|fractal|petal <id|name>"));
    };
    let within_verse = if level == Level::Verse { None } else { effective_id(request, context, Scope::Verse) };
    if request.global.dry_run {
        let plan = Plan { verb: verb_label(request), tool: Some("get_hierarchy".to_string()), method: None, path: None, args: json!({"target": query}), verse_id: within_verse };
        return result(request, context, dry_run_outcome(&plan));
    }
    let read = match read_verses(client, context, within_verse.as_deref()).await {
        Ok(read) => read,
        Err(outcome) => return result(request, context, outcome),
    };
    let entries = flatten(&read.verses);
    let label = match level {
        Level::Verse => "verse",
        Level::Fractal => "fractal",
        Level::Petal => "petal",
    };
    let found = match resolve(&entries, level, &query, within_verse.as_deref()) {
        Resolution::Found(entry) => entry,
        Resolution::Ambiguous(candidates) => {
            let listing: Vec<Value> = candidates.iter().map(|entry| json!({"id": entry.id, "name": entry.name, "verse_id": entry.verse_id})).collect();
            let mut outcome = usage_outcome("ambiguous-name", format!("`{query}` matches {} {label}s; use the id", candidates.len()));
            outcome.data = Some(json!({ "candidates": listing }));
            let mut finished = result(request, context, outcome);
            finished.context_after = Some(read.context);
            return finished;
        }
        Resolution::Unknown => {
            let mut finished = result(request, context, make_outcome(ExitCode::NotFound, "not-found", format!("no {label} with id or name `{query}`"), None));
            finished.context_after = Some(read.context);
            return finished;
        }
    };
    let mut updated = read.context;
    updated.v = 1;
    let this = NamedRef { id: found.id.clone(), name: found.name.clone() };
    match level {
        Level::Verse => {
            updated.verse = Some(this);
            updated.fractal = None;
            updated.petal = None;
        }
        Level::Fractal => {
            updated.verse = named(&entries, Level::Verse, found.verse_id.as_deref());
            updated.fractal = Some(this);
            updated.petal = None;
        }
        Level::Petal => {
            updated.verse = named(&entries, Level::Verse, found.verse_id.as_deref());
            updated.fractal = named(&entries, Level::Fractal, found.fractal_id.as_deref());
            updated.petal = Some(this);
        }
    }
    finish(request, updated)
}
