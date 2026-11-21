//! `fe petal` verbs (S3).

use serde_json::{json, Value};

use super::hierarchy::{flatten, read_verses, Level};
use super::{dry_run_outcome, effective_id, merge_args, ok_outcome, result, run_tool_verb, usage_outcome, verb_label, CommandRequest, CommandResult, Plan, Scope};
use crate::context::Context;
use crate::transport::Client;

/// Runs one `petal` verb.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    match request.verb.map(|verb| verb.verb) {
        Some("list") => list(request, client, context).await,
        Some("create") => create(request, client, context).await,
        _ => result(request, context, usage_outcome("unknown-verb", "usage: fe petal list|create")),
    }
}

/// Lists petals of the selected fractal, else of the selected verse, else of every verse.
async fn list(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let verse_id = effective_id(request, context, Scope::Verse);
    let fractal_id = effective_id(request, context, Scope::Fractal);
    if request.global.dry_run {
        let plan = Plan { verb: verb_label(request), tool: Some("get_hierarchy".to_string()), method: None, path: None, args: json!({}), verse_id };
        return result(request, context, dry_run_outcome(&plan));
    }
    let read = match read_verses(client, context, verse_id.as_deref()).await {
        Ok(read) => read,
        Err(outcome) => return result(request, context, outcome),
    };
    let petals: Vec<Value> = flatten(&read.verses)
        .into_iter()
        .filter(|entry| entry.level == Level::Petal)
        .filter(|entry| verse_id.as_deref().is_none_or(|verse| entry.verse_id.as_deref() == Some(verse)))
        .filter(|entry| fractal_id.as_deref().is_none_or(|fractal| entry.fractal_id.as_deref() == Some(fractal)))
        .map(|entry| json!({"id": entry.id, "name": entry.name, "verse_id": entry.verse_id, "fractal_id": entry.fractal_id, "nodes": entry.children}))
        .collect();
    let mut data = json!({ "petals": petals });
    if !read.errors.is_empty() {
        data["errors"] = Value::Array(read.errors);
    }
    let mut finished = result(request, context, ok_outcome(data));
    finished.context_after = Some(read.context);
    finished
}

/// `create_petal`: the fractal comes from `--fractal` or the context; the verse only selects the token.
async fn create(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe petal create <name>"));
    };
    let args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    let verse_id = effective_id(request, context, Scope::Verse);
    run_tool_verb(request, client, context, "create_petal", args, verse_id, |_| None, true).await
}
