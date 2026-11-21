//! `fe verse` verbs (S3).

use serde_json::{json, Value};

use super::hierarchy::{flatten, read_verses, Level};
use super::{created_id, dry_run_outcome, merge_args, ok_outcome, result, run_tool_verb, usage_outcome, verb_label, CommandRequest, CommandResult, Plan, Probe};
use crate::context::Context;
use crate::instance::read_tokens;
use crate::transport::Client;

/// Runs one `verse` verb.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    match request.verb.map(|verb| verb.verb) {
        Some("list") => list(request, client, context).await,
        Some("create") => create(request, client, context).await,
        _ => result(request, context, usage_outcome("unknown-verb", "usage: fe verse list|create")),
    }
}

/// Unions the hierarchy over every verse token (tokens are verse-scoped).
async fn list(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    if request.global.dry_run {
        let plan = Plan { verb: verb_label(request), tool: Some("get_hierarchy".to_string()), method: None, path: None, args: json!({}), verse_id: None };
        return result(request, context, dry_run_outcome(&plan));
    }
    let read = match read_verses(client, context, None).await {
        Ok(read) => read,
        Err(outcome) => return result(request, context, outcome),
    };
    let roles = read_tokens(&client.instance).map(|tokens| tokens.roles).unwrap_or_default();
    let verses: Vec<Value> = flatten(&read.verses)
        .into_iter()
        .filter(|entry| entry.level == Level::Verse)
        .map(|entry| json!({"id": entry.id, "name": entry.name, "role": roles.get(&entry.id), "fractals": entry.children}))
        .collect();
    let mut data = json!({ "verses": verses });
    if !read.errors.is_empty() {
        data["errors"] = Value::Array(read.errors);
    }
    let mut finished = result(request, context, ok_outcome(data));
    finished.context_after = Some(read.context);
    finished
}

/// `create_verse` is Global scope, so no verse token is selected; `--wait local` polls the token file.
async fn create(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe verse create <name>"));
    };
    let args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    run_tool_verb(request, client, context, "create_verse", args, None, |outcome| created_id(outcome.data.as_ref()).map(Probe::VerseToken), true).await
}
