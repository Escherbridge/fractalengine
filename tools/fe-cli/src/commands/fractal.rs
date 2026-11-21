//! `fe fractal` verbs (S3).

use serde_json::{json, Value};

use super::hierarchy::{flatten, read_verses, Level};
use super::{dry_run_outcome, effective_id, merge_args, ok_outcome, result, run_tool_verb, string_arg, usage_outcome, verb_label, CommandRequest, CommandResult, Plan, Scope};
use crate::context::Context;
use crate::transport::Client;

/// Runs one `fractal` verb.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    match request.verb.map(|verb| verb.verb) {
        Some("list") => list(request, client, context).await,
        Some("create") => create(request, client, context).await,
        _ => result(request, context, usage_outcome("unknown-verb", "usage: fe fractal list|create")),
    }
}

/// Lists fractals of the selected verse, or of every verse when none is selected.
async fn list(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let verse_id = effective_id(request, context, Scope::Verse);
    if request.global.dry_run {
        let plan = Plan { verb: verb_label(request), tool: Some("get_hierarchy".to_string()), method: None, path: None, args: json!({}), verse_id };
        return result(request, context, dry_run_outcome(&plan));
    }
    let read = match read_verses(client, context, verse_id.as_deref()).await {
        Ok(read) => read,
        Err(outcome) => return result(request, context, outcome),
    };
    let fractals: Vec<Value> = flatten(&read.verses)
        .into_iter()
        .filter(|entry| entry.level == Level::Fractal)
        .filter(|entry| verse_id.as_deref().is_none_or(|verse| entry.verse_id.as_deref() == Some(verse)))
        .map(|entry| json!({"id": entry.id, "name": entry.name, "verse_id": entry.verse_id, "petals": entry.children}))
        .collect();
    let mut data = json!({ "fractals": fractals });
    if !read.errors.is_empty() {
        data["errors"] = Value::Array(read.errors);
    }
    let mut finished = result(request, context, ok_outcome(data));
    finished.context_after = Some(read.context);
    finished
}

/// `create_fractal`: the verse comes from `--verse` or the context and selects the token.
async fn create(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe fractal create <name>"));
    };
    let args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    let verse_id = string_arg(&args, "verse_id");
    run_tool_verb(request, client, context, "create_fractal", args, verse_id, |_| None, true).await
}
