//! `fe node` verbs (S3).

use serde_json::{json, Map, Value};

use super::hierarchy::{petal_nodes, read_verses};
use super::{
    created_id, dry_run_outcome, effective_id, execute, make_outcome, merge_args, node_transform, ok_outcome, result, run_tool_verb, string_arg, usage_outcome, verb_label,
    CommandRequest, CommandResult, Plan, Probe, Scope,
};
use crate::context::Context;
use crate::envelope::ExitCode;
use crate::transport::Client;

/// Runs one `node` verb.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe node list|create|get|move|set|unset|delete"));
    };
    if verb.verb == "list" {
        return list(request, client, context).await;
    }
    let mut args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    let verse_id = effective_id(request, context, Scope::Verse);
    match verb.verb {
        "create" => create(request, client, context, args, verse_id).await,
        "get" => run_tool_verb(request, client, context, "read_node", args, verse_id, |_| None, false).await,
        "move" => mv(request, client, context, args, verse_id).await,
        "set" => {
            let probe = match (string_arg(&args, "node_id"), string_arg(&args, "key")) {
                (Some(node_id), Some(key)) => Some(Probe::Property { node_id, key, value: args.get("value").cloned() }),
                _ => None,
            };
            run_tool_verb(request, client, context, "set_property", args, verse_id, move |_| probe, false).await
        }
        "unset" => {
            let probe = match (string_arg(&args, "node_id"), string_arg(&args, "key")) {
                (Some(node_id), Some(key)) => Some(Probe::Property { node_id, key, value: None }),
                _ => None,
            };
            run_tool_verb(request, client, context, "delete_property", args, verse_id, move |_| probe, false).await
        }
        "delete" => {
            let probe = string_arg(&args, "node_id").map(Probe::NodeGone);
            args.retain(|_, value| !value.is_null());
            run_tool_verb(request, client, context, "delete_node", args, verse_id, move |_| probe, false).await
        }
        _ => result(request, context, usage_outcome("unknown-verb", "usage: fe node list|create|get|move|set|unset|delete")),
    }
}

/// Percent-encodes everything outside RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `--kind` pages the typed REST list; without it the petal's nodes come from one hierarchy read.
async fn list(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "usage: fe node list"));
    };
    let args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    let Some(petal_id) = string_arg(&args, "petal_id") else {
        return result(request, context, usage_outcome("missing-context", "no petal selected; run `fe use petal <id|name>` or pass --petal"));
    };
    let verse_id = effective_id(request, context, Scope::Verse);
    if let Some(kind) = string_arg(&args, "kind") {
        if let Err(error) = crate::ids::require_route_id("--petal", &petal_id) {
            return result(request, context, crate::commands::from_cli_error(error));
        }
        let mut path = format!("/petals/{}/nodes?kind={}", percent_encode(&petal_id), percent_encode(&kind));
        for key in ["path_id", "cursor", "limit"] {
            if let Some(value) = string_arg(&args, key) {
                path.push_str(&format!("&{key}={}", percent_encode(&value)));
            }
        }
        let plan = Plan { verb: verb_label(request), tool: None, method: Some("GET".to_string()), path: Some(path), args: json!({}), verse_id };
        let outcome = execute(&plan, client, request).await;
        return result(request, context, outcome);
    }
    if request.global.dry_run {
        let plan = Plan { verb: verb_label(request), tool: Some("get_hierarchy".to_string()), method: None, path: None, args: json!({"petal_id": petal_id}), verse_id };
        return result(request, context, dry_run_outcome(&plan));
    }
    match read_verses(client, context, verse_id.as_deref()).await {
        Err(outcome) => result(request, context, outcome),
        Ok(read) => {
            let nodes = petal_nodes(&read.verses, &petal_id);
            let mut finished = result(request, context, ok_outcome(json!({"petal_id": petal_id, "nodes": nodes})));
            finished.context_after = Some(read.context);
            finished
        }
    }
}

/// `create_node`, or `place_asset` when an asset id is given.
async fn create(request: &CommandRequest, client: &Client, context: &Context, mut args: Map<String, Value>, verse_id: Option<String>) -> CommandResult {
    if let Some(asset) = args.remove("asset") {
        args.entry("asset_id").or_insert(asset);
    }
    let tool = if args.contains_key("asset_id") { "place_asset" } else { "create_node" };
    if tool == "create_node" && (args.contains_key("rotation") || args.contains_key("scale")) {
        return result(request, context, usage_outcome("invalid-args", "rotation and scale need --asset-id; create the node, then use `fe node move`"));
    }
    run_tool_verb(request, client, context, tool, args, verse_id, |outcome| created_id(outcome.data.as_ref()).map(Probe::NodeExists), true).await
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn vector3(value: Option<&Value>) -> Option<[f64; 3]> {
    let items = value?.as_array()?;
    match items.as_slice() {
        [x, y, z] => Some([number(x)?, number(y)?, number(z)?]),
        _ => None,
    }
}

fn to_json(vector: [f64; 3]) -> Value {
    json!([vector[0], vector[1], vector[2]])
}

/// `update_transform` needs all three vectors: missing parts are read from the node first.
async fn mv(request: &CommandRequest, client: &Client, context: &Context, mut args: Map<String, Value>, verse_id: Option<String>) -> CommandResult {
    let axes: [Option<f64>; 3] = ["x", "y", "z"].map(|axis| args.remove(axis).as_ref().and_then(number));
    let position = vector3(args.get("position"));
    let rotation = vector3(args.get("rotation"));
    let scale = vector3(args.get("scale"));
    let Some(node_id) = string_arg(&args, "node_id") else {
        return result(request, context, usage_outcome("invalid-args", "node_id is required"));
    };
    let position_complete = position.is_some() || axes.iter().all(Option::is_some);
    let needs_read = !(position_complete && rotation.is_some() && scale.is_some());
    let mut current = ([0.0; 3], [0.0; 3], [1.0; 3]);
    let dry_run = request.global.dry_run;
    if needs_read && !dry_run {
        let reply = client.call_tool("read_node", json!({"node_id": node_id}), verse_id.as_deref()).await;
        if reply.exit != ExitCode::Ok {
            return result(request, context, reply);
        }
        match reply.data.as_ref().and_then(node_transform) {
            Some(read) => current = read,
            None => return result(request, context, make_outcome(ExitCode::ServerFault, "unexpected-reply", "read_node reply has no readable transform", None)),
        }
    }
    let mut final_position = position.unwrap_or(current.0);
    for (index, axis) in axes.iter().enumerate() {
        if let Some(component) = axis {
            final_position[index] = *component;
        }
    }
    let final_rotation = rotation.unwrap_or(current.1);
    let final_scale = scale.unwrap_or(current.2);
    args.insert("position".to_string(), to_json(final_position));
    args.insert("rotation".to_string(), to_json(final_rotation));
    args.insert("scale".to_string(), to_json(final_scale));
    let probe = Probe::Transform { node_id, position: Some(final_position), rotation: Some(final_rotation), scale: Some(final_scale) };
    let mut finished = run_tool_verb(request, client, context, "update_transform", args, verse_id, move |_| Some(probe), false).await;
    if needs_read && dry_run {
        if let Some(Value::Object(data)) = finished.outcome.data.as_mut() {
            data.insert("unspecified_parts".to_string(), json!("filled with defaults here; a real run reads them from the node first"));
        }
    }
    finished
}
