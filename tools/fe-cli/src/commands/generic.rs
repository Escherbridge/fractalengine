//! Phase-2 verbs: every catalog row with `phase == 2` runs through here (see AGENTS.md §phase-2 verbs).

use std::io::Read;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use super::{effective_id, execute, from_cli_error, merge_args, result, run_tool_verb, string_arg, usage_outcome, verb_label, CommandRequest, CommandResult, Plan, Scope};
use crate::catalog::{self, Door};
use crate::context::Context;
use crate::transport::{Client, Outcome};
use crate::{ids, secrets};

/// Largest GLB the MCP `upload_asset` door takes.
const MAX_UPLOAD_BYTES: u64 = 11 * 1024 * 1024;

/// Runs one phase-2 row: REST rows build a path, MCP rows go through `run_tool_verb` after `prepare`.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    let Some(verb) = request.verb else {
        return result(request, context, usage_outcome("unknown-verb", "run `fe help`"));
    };
    let mut args = match merge_args(request, context, verb) {
        Ok(args) => args,
        Err(outcome) => return result(request, context, outcome),
    };
    let tool = match verb.door {
        Door::Mcp(tool) => tool,
        Door::Rest(method, template) => return rest_verb(request, client, context, method, template, args).await,
        Door::Local => return result(request, context, usage_outcome("unknown-verb", "run `fe help`")),
    };
    if let Err(outcome) = prepare(request, client, verb, &mut args).await {
        return result(request, context, outcome);
    }
    // Global and token-scoped tools pick their own bearer; the rest use the verse the context (or --verse) names.
    let scope = catalog::verb_scope(verb);
    let verse_id = request
        .global
        .verse
        .clone()
        .or_else(|| string_arg(&args, "verse_id"))
        .or_else(|| if matches!(scope, Some("global" | "token")) { None } else { effective_id(request, context, Scope::Verse) });
    let creates_id = (verb.noun, verb.verb) == ("asset", "upload");
    run_tool_verb(request, client, context, tool, args, verse_id, |_| None, creates_id).await
}

/// Per-verb argument shaping that the schema cannot express.
async fn prepare(request: &CommandRequest, client: &Client, verb: &catalog::Verb, args: &mut Map<String, Value>) -> Result<(), Outcome> {
    match (verb.noun, verb.verb) {
        ("asset", "upload") => prepare_upload(request, client, args),
        ("terrain", "set") if args.get("terrain").is_none_or(Value::is_null) => {
            Err(usage_outcome("missing-required", "terrain set needs --terrain-json <TerrainConfig> or --args @file; use `fe terrain clear` to remove it"))
        }
        ("terrain", "clear") => {
            args.insert("terrain".to_string(), Value::Null);
            Ok(())
        }
        ("group", "delete") => {
            args.insert("delete".to_string(), Value::Bool(true));
            Ok(())
        }
        ("invite", "create") => prepare_invite(request, args).await,
        ("join", _) => prepare_join(request, args),
        _ => Ok(()),
    }
}

/// `GET` rows (`terrain get`): the route id is validated before it enters the path.
async fn rest_verb(request: &CommandRequest, client: &Client, context: &Context, method: &str, template: &str, args: Map<String, Value>) -> CommandResult {
    let Some(petal_id) = string_arg(&args, "petal_id") else {
        return result(request, context, usage_outcome("missing-context", "no petal selected; run `fe use petal <id|name>` or pass --petal"));
    };
    if let Err(error) = ids::require_route_id("--petal", &petal_id) {
        return result(request, context, from_cli_error(error));
    }
    let path = template.replace("{petal_id}", &petal_id);
    let verse_id = effective_id(request, context, Scope::Verse);
    let plan = Plan { verb: verb_label(request), tool: None, method: Some(method.to_string()), path: Some(path), args: json!({}), verse_id };
    let outcome = execute(&plan, client, request).await;
    let mut finished = result(request, context, outcome);
    finished.resolved_args = Some(json!({ "petal_id": petal_id }));
    finished
}

/// `--file` becomes `data_base64`; the instance's own token file is never uploadable.
fn prepare_upload(request: &CommandRequest, client: &Client, args: &mut Map<String, Value>) -> Result<(), Outcome> {
    let Some(file) = string_arg(args, "file") else {
        return Err(usage_outcome("missing-required", "usage: fe asset upload --file <path.glb> [--name <display name>]"));
    };
    args.remove("file");
    let given = PathBuf::from(&file);
    let path = if given.is_absolute() { given } else { request.cwd.join(given) };
    if let (Ok(wanted), Ok(token_file)) = (std::fs::canonicalize(&path), std::fs::canonicalize(&client.instance.tokens_path)) {
        if wanted == token_file {
            return Err(from_cli_error(secrets::token_in_args()));
        }
    }
    let size = std::fs::metadata(&path)
        .map_err(|error| usage_outcome("invalid-input", format!("cannot read {file}: {}", error.kind())))?
        .len();
    if size > MAX_UPLOAD_BYTES {
        return Err(usage_outcome("invalid-input", format!("{file} is {size} bytes; the MCP door takes at most {MAX_UPLOAD_BYTES} (use the REST multipart upload for larger GLBs)")));
    }
    if string_arg(args, "name").is_none() {
        let display = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "asset.glb".to_string());
        args.insert("name".to_string(), Value::String(display));
    }
    let encoded = if request.global.dry_run {
        format!("<{size} bytes, base64 elided in a dry run>")
    } else {
        let bytes = std::fs::read(&path).map_err(|error| usage_outcome("invalid-input", format!("cannot read {file}: {}", error.kind())))?;
        base64_encode(&bytes)
    };
    args.insert("data_base64".to_string(), Value::String(encoded));
    Ok(())
}

/// `--to-instance <name>` becomes the recipient DID read from that instance's `get_identity`.
async fn prepare_invite(request: &CommandRequest, args: &mut Map<String, Value>) -> Result<(), Outcome> {
    let target = string_arg(args, "to_instance");
    args.remove("to_instance");
    match (target, args.contains_key("recipient_did")) {
        (Some(_), true) => Err(usage_outcome("invalid-args", "--to-instance and --recipient-did are alternatives; pass one")),
        (None, false) => Err(usage_outcome("missing-required", "usage: fe invite create --to-instance <name> | --recipient-did <did:key> [--role viewer|editor|manager]")),
        (None, true) => Ok(()),
        (Some(name), false) => {
            let did = if request.global.dry_run { format!("<did of {name}>") } else { identity_did(request, &name).await? };
            args.insert("recipient_did".to_string(), Value::String(did));
            Ok(())
        }
    }
}

/// The DID another local instance reports for itself.
async fn identity_did(request: &CommandRequest, selector: &str) -> Result<String, Outcome> {
    let instance = crate::instance::resolve(Some(selector), &request.env, &request.cwd).map_err(from_cli_error)?;
    let peer = Client::new(instance).map_err(from_cli_error)?;
    let reply = peer.call_tool("get_identity", json!({}), None).await;
    if reply.exit != crate::envelope::ExitCode::Ok {
        return Err(reply);
    }
    reply
        .data
        .as_ref()
        .and_then(|data| data.get("did").or_else(|| data.get("data").and_then(|inner| inner.get("did"))))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| usage_outcome("invalid-input", format!("instance {selector} reported no DID")))
}

/// `--invite -` reads stdin and `--invite @file` reads a file, so the capability never sits in a process listing.
fn prepare_join(request: &CommandRequest, args: &mut Map<String, Value>) -> Result<(), Outcome> {
    let Some(raw) = string_arg(args, "invite") else {
        return Err(usage_outcome("missing-required", "usage: fe join --invite <fe-join-v2:…|-|@file>"));
    };
    let invite = if raw == "-" {
        if request.global.dry_run {
            "<invite read from stdin>".to_string()
        } else {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|error| usage_outcome("invalid-input", format!("cannot read stdin: {}", error.kind())))?;
            text
        }
    } else if let Some(file) = raw.strip_prefix('@') {
        let given = PathBuf::from(file);
        let path = if given.is_absolute() { given } else { request.cwd.join(given) };
        std::fs::read_to_string(&path).map_err(|error| usage_outcome("invalid-input", format!("cannot read {file}: {}", error.kind())))?
    } else {
        raw
    };
    let invite = invite.trim().to_string();
    if invite.is_empty() {
        return Err(usage_outcome("invalid-input", "the invite is empty"));
    }
    args.insert("invite".to_string(), Value::String(invite));
    Ok(())
}

/// Standard base64 with padding (RFC 4648 §4); the CLI links no base64 crate.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(word >> 18) as usize & 63] as char);
        out.push(ALPHABET[(word >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(word >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[word as usize & 63] as char } else { '=' });
    }
    out
}
