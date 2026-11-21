//! Core verb dispatch and the helpers every noun module shares (S3).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::catalog::{self, Verb};
use crate::context::Context;
use crate::envelope::{CliError, EnvelopeContext, ExitCode, Refusal};
use crate::instance::read_tokens;
use crate::transport::{Client, Outcome};
use crate::Env;

pub mod fractal;
pub mod generic;
pub mod hierarchy;
pub mod instance;
pub mod node;
pub mod note;
pub mod petal;
pub mod replay;
pub mod scenario;
pub mod use_ctx;
pub mod verse;

/// Flags valid on every verb.
#[derive(Debug, Clone, Default)]
pub struct GlobalFlags {
    pub json: bool,
    pub dry_run: bool,
    /// `local` or `peers[=a,b]`.
    pub wait: Option<String>,
    /// `wait` came from `FE_WAIT_DEFAULT`: a harness default degrades to local when no peer is discoverable.
    pub wait_default: bool,
    pub wait_budget_ms: u64,
    pub retry: u32,
    pub verse: Option<String>,
    pub fractal: Option<String>,
    pub petal: Option<String>,
    /// An unsupported `FE_WAIT_DEFAULT` value that was downgraded to `local` (reported in the envelope `wait`).
    pub wait_fallback: Option<String>,
}

/// A parsed verb invocation.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    /// First command word (`x` has no table row).
    pub noun: String,
    /// The routed table row; `None` only for `x`.
    pub verb: Option<&'static Verb>,
    /// Verb-specific arguments after flag parsing.
    pub args: BTreeMap<String, Value>,
    /// Remaining positional words.
    pub positionals: Vec<String>,
    pub global: GlobalFlags,
    /// Injected environment (peer discovery, `fe replay` child runs).
    pub env: Env,
    /// Working directory for `.qa/` discovery and relative selectors.
    pub cwd: PathBuf,
    /// `--instance` as typed (replay's default instance for unmapped agents).
    pub instance_flag: Option<String>,
}

/// What a verb hands back to the spine.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Final tool arguments (positionals and context merged); the journal records them for replay and scenario emission.
    pub resolved_args: Option<Value>,
    pub outcome: Outcome,
    /// Context after the verb (written back when changed).
    pub context_after: Option<Context>,
    pub created_ids: Vec<String>,
    pub resolved_context: EnvelopeContext,
    pub wait: Option<Value>,
}

/// Default `--wait-budget` when the flag is absent or zero.
const DEFAULT_WAIT_BUDGET_MS: u64 = 30_000;
/// Spacing of read-back polls under `--wait local`.
const WAIT_POLL_MS: u64 = 150;

/// Routes a request to its noun module.
pub async fn dispatch(request: &CommandRequest, client: &Client, context: &Context) -> CommandResult {
    if request.verb.is_none() {
        return result(request, context, usage_outcome("unknown-command", format!("unknown command `{}`; run `fe help`", request.noun)));
    }
    if let Some(wait) = request.global.wait.as_deref() {
        if parse_wait(wait).is_none() {
            return result(request, context, usage_outcome("wait-unsupported", format!("--wait {wait} is not a wait mode; use local, peers or peers=<name>,<name>")));
        }
    }
    if let Err(outcome) = check_peers(request, client) {
        return result(request, context, outcome);
    }
    if request.verb.is_some_and(|row| row.phase == 2) {
        return generic::run(request, client, context).await;
    }
    match request.noun.as_str() {
        "use" | "ctx" => use_ctx::run(request, client, context).await,
        "verse" => verse::run(request, client, context).await,
        "fractal" => fractal::run(request, client, context).await,
        "petal" => petal::run(request, client, context).await,
        "node" => node::run(request, client, context).await,
        // `help`, `note` and `instance` never reach here: the spine answers them without an instance.
        other => result(request, context, usage_outcome("unknown-command", format!("`{other}` is answered by the spine, not dispatch; run `fe help`"))),
    }
}

// ---------------------------------------------------------------------------
// Outcomes and results
// ---------------------------------------------------------------------------

/// Builds an outcome with no retry hint or refusal detail.
pub(crate) fn make_outcome(exit: ExitCode, code: &str, message: impl Into<String>, data: Option<Value>) -> Outcome {
    Outcome { exit, code: code.to_string(), message: message.into(), data, retry_after_ms: None, refusal: Refusal::default(), http_status: None, pre_admission: false }
}

/// Success outcome carrying `data`.
pub(crate) fn ok_outcome(data: Value) -> Outcome {
    make_outcome(ExitCode::Ok, "ok", "", Some(data))
}

/// Exit-6 outcome.
pub(crate) fn usage_outcome(code: &str, message: impl Into<String>) -> Outcome {
    make_outcome(ExitCode::Usage, code, message, None)
}

/// Converts a spine-level error into an outcome.
pub(crate) fn from_cli_error(error: CliError) -> Outcome {
    make_outcome(error.exit, &error.code, error.message, None)
}

/// Which context level an id belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Verse,
    Fractal,
    Petal,
}

impl Scope {
    fn from_field(field: &str) -> Option<Scope> {
        match field {
            "verse" => Some(Scope::Verse),
            "fractal" => Some(Scope::Fractal),
            "petal" => Some(Scope::Petal),
            _ => None,
        }
    }
}

/// Explicit `--verse/--fractal/--petal` wins over the context.
pub(crate) fn effective_id(request: &CommandRequest, context: &Context, scope: Scope) -> Option<String> {
    let (flag, named) = match scope {
        Scope::Verse => (&request.global.verse, &context.verse),
        Scope::Fractal => (&request.global.fractal, &context.fractal),
        Scope::Petal => (&request.global.petal, &context.petal),
    };
    flag.clone().or_else(|| named.as_ref().map(|reference| reference.id.clone())).filter(|id| !id.trim().is_empty())
}

/// Context ids echoed in the envelope.
pub(crate) fn envelope_context(request: &CommandRequest, context: &Context) -> EnvelopeContext {
    EnvelopeContext {
        instance: context.instance.clone(),
        verse_id: effective_id(request, context, Scope::Verse),
        fractal_id: effective_id(request, context, Scope::Fractal),
        petal_id: effective_id(request, context, Scope::Petal),
    }
}

/// Wraps an outcome with the resolved context; callers fill the optional fields.
pub(crate) fn result(request: &CommandRequest, context: &Context, outcome: Outcome) -> CommandResult {
    CommandResult { outcome, context_after: None, created_ids: Vec::new(), resolved_context: envelope_context(request, context), wait: None, resolved_args: None }
}

/// The `noun verb` label of a request.
pub(crate) fn verb_label(request: &CommandRequest) -> String {
    match request.verb {
        Some(verb) if verb.verb.is_empty() => verb.noun.to_string(),
        Some(verb) => format!("{} {}", verb.noun, verb.verb),
        None => String::new(),
    }
}

/// The id of a created entity from a create reply (`{id}`, REST `{data:{id}}`, or an upload's `{asset_id}`).
pub(crate) fn created_id(data: Option<&Value>) -> Option<String> {
    let data = data?;
    data.get("id")
        .or_else(|| data.get("data").and_then(|inner| inner.get("id")))
        .or_else(|| data.get("asset_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Time (RFC3339 UTC, millisecond precision, no external crates)
// ---------------------------------------------------------------------------

fn unix_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

/// Howard Hinnant's days-from-civil algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = (if shifted >= 0 { shifted } else { shifted - 146_096 }) / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub(crate) fn now_rfc3339() -> String {
    let millis = unix_millis();
    let seconds = millis.div_euclid(1000);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let second_of_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60,
        millis.rem_euclid(1000)
    )
}

fn parse_rfc3339_millis(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let rest = text.get(19..)?;
    let (fraction_millis, tail) = match rest.strip_prefix('.') {
        Some(after) => {
            let digits = after.chars().take_while(char::is_ascii_digit).count();
            let padded: String = after[..digits].chars().chain("000".chars()).take(3).collect();
            (padded.parse::<i64>().ok()?, &after[digits..])
        }
        None => (0, rest),
    };
    if tail != "Z" {
        return None;
    }
    Some((days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + fraction_millis)
}

/// Milliseconds since an RFC3339 UTC timestamp (`None` when unparseable).
pub(crate) fn rfc3339_age_millis(text: &str) -> Option<i64> {
    parse_rfc3339_millis(text).map(|then| unix_millis() - then)
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// Parses valid non-null JSON, else keeps the text as a string.
pub(crate) fn json_or_string(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(value) if !value.is_null() => value,
        _ => Value::String(text.to_string()),
    }
}

/// Turns `"x,y,z"` or an array of numeric strings into a number array.
pub(crate) fn normalize_vector(value: &Value) -> Result<Value, String> {
    let parse = |text: &str| -> Result<Value, String> {
        let number: f64 = text.trim().parse().map_err(|_| format!("`{text}` is not a number"))?;
        serde_json::Number::from_f64(number).map(Value::Number).ok_or_else(|| format!("`{text}` is not finite"))
    };
    match value {
        Value::String(text) => text.split(',').map(parse).collect::<Result<Vec<_>, _>>().map(Value::Array),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => parse(text),
                other => Ok(other.clone()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        other => Ok(other.clone()),
    }
}

/// Builds the tool argument object: flags, then bare words, then context ids.
pub(crate) fn merge_args(request: &CommandRequest, context: &Context, verb: &Verb) -> Result<Map<String, Value>, Outcome> {
    let detail = catalog::detail(verb);
    let mut map: Map<String, Value> = request.args.iter().map(|(key, value)| (key.clone(), value.clone())).collect();
    let schema_properties: Vec<String> = match verb.door {
        catalog::Door::Mcp(tool) => catalog::tool_row(tool)
            .and_then(|row| row.input_schema.get("properties").and_then(Value::as_object).map(|properties| properties.keys().cloned().collect()))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let known = |name: &str| schema_properties.iter().any(|property| property == name) || detail.positionals.contains(&name);
    for key in map.keys().cloned().collect::<Vec<_>>() {
        let full = format!("{key}_id");
        if known(&full) && !map.contains_key(&full) {
            if let Some(value) = map.remove(&key) {
                map.insert(full, value);
            }
        }
    }
    for (index, name) in detail.positionals.iter().enumerate() {
        if map.contains_key(*name) {
            continue;
        }
        let Some(word) = request.positionals.get(index) else { continue };
        let text = if matches!(*name, "value" | "sql") { request.positionals[index..].join(" ") } else { word.clone() };
        map.insert((*name).to_string(), Value::String(text));
    }
    for (argument, field) in detail.from_context {
        if map.contains_key(*argument) {
            continue;
        }
        let Some(scope) = Scope::from_field(field) else { continue };
        match effective_id(request, context, scope) {
            Some(id) => {
                map.insert((*argument).to_string(), Value::String(id));
            }
            None => {
                return Err(usage_outcome(
                    "missing-context",
                    format!("no {field} selected; run `fe use {field} <id|name>` or pass --{field}"),
                ))
            }
        }
    }
    for key in ["position", "rotation", "scale"] {
        if let Some(value) = map.get(key) {
            let normalized = normalize_vector(value).map_err(|message| usage_outcome("invalid-args", format!("invalid {key}: {message}")))?;
            map.insert(key.to_string(), normalized);
        }
    }
    if let Some(Value::String(text)) = map.get("value") {
        let parsed = json_or_string(text);
        map.insert("value".to_string(), parsed);
    }
    Ok(map)
}

/// A string argument by name (numbers are stringified).
pub(crate) fn string_arg(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Planning, execution, dry-run
// ---------------------------------------------------------------------------

/// One resolved outbound request.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub verb: String,
    pub tool: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub args: Value,
    pub verse_id: Option<String>,
}

impl Plan {
    fn to_json(&self) -> Value {
        let via = if self.tool.is_some() { "mcp" } else { "rest" };
        json!({
            "verb": self.verb,
            "via": via,
            "tool": self.tool,
            "method": self.method,
            "path": self.path,
            "args": crate::journal::redact(&self.args),
            "verse_id": self.verse_id,
        })
    }
}

/// Validates MCP arguments against the snapshot schema and builds the plan.
pub(crate) fn tool_plan(request: &CommandRequest, tool: &str, args: Map<String, Value>, verse_id: Option<String>) -> Result<Plan, Outcome> {
    let Some(row) = catalog::tool_row(tool) else {
        return Err(make_outcome(ExitCode::Internal, "unknown-tool", format!("tool {tool} is not in the catalog snapshot"), None));
    };
    let args = Value::Object(args);
    catalog::validate_args(&row.input_schema, &args).map_err(|message| usage_outcome("invalid-args", message))?;
    Ok(Plan { verb: verb_label(request), tool: Some(tool.to_string()), method: None, path: None, args, verse_id })
}

/// The dry-run reply: the planned request, nothing sent.
pub(crate) fn dry_run_outcome(plan: &Plan) -> Outcome {
    let mut outcome = ok_outcome(json!({"dry_run": true, "planned": plan.to_json()}));
    outcome.message = "dry run: nothing sent".to_string();
    outcome
}

/// Sends a plan once, or prints it under `--dry-run`; `--retry` is the spine's (`transport::retry_allowed`).
pub(crate) async fn execute(plan: &Plan, client: &Client, request: &CommandRequest) -> Outcome {
    if request.global.dry_run {
        return dry_run_outcome(plan);
    }
    match &plan.tool {
        Some(tool) => client.call_tool(tool, plan.args.clone(), plan.verse_id.as_deref()).await,
        None => {
            let method = plan.method.as_deref().unwrap_or("GET");
            let body = if method == "GET" { None } else { Some(plan.args.clone()) };
            client.rest(method, plan.path.as_deref().unwrap_or("/"), body, plan.verse_id.as_deref()).await
        }
    }
}

// ---------------------------------------------------------------------------
// --wait local
// ---------------------------------------------------------------------------

/// What proves a write is observable on the same instance.
#[derive(Debug, Clone)]
pub(crate) enum Probe {
    /// The verse id appears in the token file.
    VerseToken(String),
    NodeExists(String),
    NodeGone(String),
    Transform { node_id: String, position: Option<[f64; 3]>, rotation: Option<[f64; 3]>, scale: Option<[f64; 3]> },
    Property { node_id: String, key: String, value: Option<Value> },
}

fn array3(value: &Value, default: [f64; 3]) -> [f64; 3] {
    match value.as_array() {
        Some(items) if items.len() == 3 => [items[0].as_f64().unwrap_or(default[0]), items[1].as_f64().unwrap_or(default[1]), items[2].as_f64().unwrap_or(default[2])],
        _ => default,
    }
}

/// `(position, rotation, scale)` of a `read_node` reply (row stores x/z as a point and y as elevation).
pub(crate) fn node_transform(data: &Value) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let row = data.get("data").filter(|inner| inner.is_object()).unwrap_or(data);
    let position = match row.get("position")?.get("coordinates").and_then(Value::as_array) {
        Some(coordinates) if coordinates.len() >= 2 => [
            coordinates[0].as_f64()?,
            row.get("elevation").and_then(Value::as_f64).unwrap_or(0.0),
            coordinates[1].as_f64()?,
        ],
        _ => array3(row.get("position")?, [f64::NAN; 3]),
    };
    if position.iter().any(|component| component.is_nan()) {
        return None;
    }
    Some((position, array3(row.get("rotation").unwrap_or(&Value::Null), [0.0; 3]), array3(row.get("scale").unwrap_or(&Value::Null), [1.0; 3])))
}

fn close_enough(expected: Option<[f64; 3]>, actual: [f64; 3]) -> bool {
    expected.is_none_or(|wanted| wanted.iter().zip(actual).all(|(left, right)| (left - right).abs() < 1e-3))
}

async fn probe_settled(client: &Client, verse_id: Option<&str>, probe: &Probe) -> bool {
    match probe {
        Probe::VerseToken(id) => read_tokens(&client.instance).map(|tokens| tokens.tokens.contains_key(id)).unwrap_or(false),
        Probe::NodeExists(id) => client.call_tool("read_node", json!({"node_id": id}), verse_id).await.exit == ExitCode::Ok,
        Probe::NodeGone(id) => {
            // Without a token for the verse the uniform not-found is "cannot see", not "gone".
            if let Some(verse) = verse_id {
                if !read_tokens(&client.instance).map(|tokens| tokens.tokens.contains_key(verse)).unwrap_or(false) {
                    return false;
                }
            }
            client.call_tool("read_node", json!({"node_id": id}), verse_id).await.exit == ExitCode::NotFound
        }
        Probe::Transform { node_id, position, rotation, scale } => {
            let reply = client.call_tool("read_node", json!({"node_id": node_id}), verse_id).await;
            match reply.data.as_ref().filter(|_| reply.exit == ExitCode::Ok).and_then(node_transform) {
                Some((actual_position, actual_rotation, actual_scale)) => {
                    close_enough(*position, actual_position) && close_enough(*rotation, actual_rotation) && close_enough(*scale, actual_scale)
                }
                None => false,
            }
        }
        Probe::Property { node_id, key, value } => {
            let reply = client.call_tool("get_properties", json!({"node_id": node_id}), verse_id).await;
            if reply.exit != ExitCode::Ok {
                return false;
            }
            let Some(data) = reply.data.as_ref() else { return false };
            let properties = data.get("properties").unwrap_or(data);
            match (properties.get(key), value) {
                (Some(actual), Some(wanted)) => actual == wanted,
                (None, None) => true,
                _ => false,
            }
        }
    }
}

/// A parsed `--wait` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitMode {
    Local,
    /// Peers named by `peers=a,b`; empty means every other instance in `.qa/peers-current`.
    Peers(Vec<String>),
}

/// `local`, `peers` or `peers=<name>,<name>`; anything else is `None`.
pub(crate) fn parse_wait(text: &str) -> Option<WaitMode> {
    match text.trim() {
        "local" => Some(WaitMode::Local),
        "peers" => Some(WaitMode::Peers(Vec::new())),
        other => {
            let names: Vec<String> = other.strip_prefix("peers=")?.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect();
            (!names.is_empty()).then_some(WaitMode::Peers(names))
        }
    }
}

/// The request's wait mode when the verb is a write that will wait (reads and dry runs never wait).
fn active_wait(request: &CommandRequest) -> Option<WaitMode> {
    if request.global.dry_run || !request.verb.is_some_and(catalog::is_write) {
        return None;
    }
    request.global.wait.as_deref().and_then(parse_wait)
}

/// Clients for the other instances of `.qa/peers-current` (own instance excluded; `names` filters when non-empty).
fn peer_clients(request: &CommandRequest, client: &Client, names: &[String]) -> Result<Vec<(String, Client)>, Outcome> {
    let found = crate::instance::list(&request.env, &request.cwd).map_err(from_cli_error)?;
    let own = &client.instance;
    let mut peers = Vec::new();
    for instance in found {
        // Peers on one machine have distinct token files (and ports); the token file is the identity.
        let is_own = instance.tokens_path == own.tokens_path
            || matches!((std::fs::canonicalize(&instance.tokens_path), std::fs::canonicalize(&own.tokens_path)), (Ok(left), Ok(right)) if left == right);
        if is_own || (!names.is_empty() && !names.contains(&instance.name)) {
            continue;
        }
        let name = instance.name.clone();
        peers.push((name, Client::new(instance).map_err(from_cli_error)?));
    }
    Ok(peers)
}

/// Refuses an explicit `--wait peers` that has no peer to wait for, before the write is sent (exit 6).
fn check_peers(request: &CommandRequest, client: &Client) -> Result<(), Outcome> {
    let Some(WaitMode::Peers(names)) = active_wait(request) else { return Ok(()) };
    let peers = peer_clients(request, client, &names)?;
    let missing: Vec<&String> = names.iter().filter(|name| **name != client.instance.name && !peers.iter().any(|(peer, _)| peer == *name)).collect();
    if !missing.is_empty() {
        let listed: Vec<&str> = missing.iter().map(|name| name.as_str()).collect();
        return Err(usage_outcome("unknown-peer", format!("no peer instance named {} in .qa/peers-current; try `fe instance list`", listed.join(", "))));
    }
    if peers.is_empty() && !request.global.wait_default {
        return Err(usage_outcome("no-peers", "--wait peers found no other instance in .qa/peers-current; try `fe instance list`"));
    }
    Ok(())
}

/// Polls the probe on this instance (`--wait local`) and on every peer (`--wait peers`) until it holds everywhere or
/// `--wait-budget` expires (exit 9, the write still landed); see AGENTS.md §wait-peers.
pub(crate) async fn settle(mut outcome: Outcome, request: &CommandRequest, client: &Client, verse_id: Option<&str>, probe: Probe) -> (Outcome, Option<Value>) {
    let Some(mode) = active_wait(request) else { return (outcome, None) };
    if outcome.exit != ExitCode::Ok {
        return (outcome, None);
    }
    let mut note: Option<String> = None;
    let mut peers: Vec<(String, Client)> = Vec::new();
    if let WaitMode::Peers(names) = &mode {
        if matches!(probe, Probe::VerseToken(_)) {
            note = Some("a created verse reaches a peer only through an invite; waited with local".to_string());
        } else {
            match peer_clients(request, client, names) {
                Ok(found) => peers = found,
                Err(failure) => return (failure, None),
            }
            if peers.is_empty() {
                note = Some("FE_WAIT_DEFAULT=peers found no other instance in .qa/peers-current; waited with local".to_string());
            }
        }
    }
    let label = if peers.is_empty() { "local" } else { "peers" };
    let budget_ms = if request.global.wait_budget_ms == 0 { DEFAULT_WAIT_BUDGET_MS } else { request.global.wait_budget_ms };
    let started = Instant::now();
    let mut polls = 0u32;
    let mut local_ms: Option<u64> = None;
    let mut peer_ms: Vec<Option<u64>> = vec![None; peers.len()];
    let report = |settled: bool, polls: u32, local_ms: Option<u64>, peer_ms: &[Option<u64>], peers: &[(String, Client)], elapsed_ms: u64| {
        let mut wait = json!({"mode": label, "settled": settled, "ms": elapsed_ms, "polls": polls});
        if !peers.is_empty() {
            wait["local_ms"] = json!(local_ms);
            wait["peers"] = Value::Array(peers.iter().zip(peer_ms).map(|((name, _), ms)| json!({"name": name, "settled": ms.is_some(), "ms": ms})).collect());
        }
        if let Some(text) = &note {
            wait["requested"] = json!("peers");
            wait["note"] = json!(text);
        }
        wait
    };
    loop {
        polls += 1;
        if local_ms.is_none() && probe_settled(client, verse_id, &probe).await {
            local_ms = Some(started.elapsed().as_millis() as u64);
        }
        for (index, (_, peer)) in peers.iter().enumerate() {
            if peer_ms[index].is_none() && probe_settled(peer, verse_id, &probe).await {
                peer_ms[index] = Some(started.elapsed().as_millis() as u64);
            }
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if local_ms.is_some() && peer_ms.iter().all(Option::is_some) {
            return (outcome, Some(report(true, polls, local_ms, &peer_ms, &peers, elapsed_ms)));
        }
        if elapsed_ms >= budget_ms {
            let unseen: Vec<&str> = peers.iter().zip(&peer_ms).filter(|(_, ms)| ms.is_none()).map(|((name, _), _)| name.as_str()).collect();
            outcome.exit = ExitCode::WaitExpired;
            outcome.code = "wait-expired".to_string();
            outcome.message = if local_ms.is_none() {
                format!("write accepted but not observable within {budget_ms} ms; reconcile by reading")
            } else {
                format!("write accepted but not seen on peer {} within {budget_ms} ms; reconcile by reading", unseen.join(", "))
            };
            return (outcome, Some(report(false, polls, local_ms, &peer_ms, &peers, elapsed_ms)));
        }
        tokio::time::sleep(Duration::from_millis(WAIT_POLL_MS)).await;
    }
}

/// Plans, executes and (for writes) settles one MCP-backed verb; `verse_id` picks the token.
pub(crate) async fn run_tool_verb(
    request: &CommandRequest,
    client: &Client,
    context: &Context,
    tool: &str,
    args: Map<String, Value>,
    verse_id: Option<String>,
    probe: impl FnOnce(&Outcome) -> Option<Probe>,
    created: bool,
) -> CommandResult {
    let plan = match tool_plan(request, tool, args, verse_id.clone()) {
        Ok(plan) => plan,
        Err(outcome) => return result(request, context, outcome),
    };
    let resolved_args = plan.args.clone();
    let mut outcome = execute(&plan, client, request).await;
    let mut wait = None;
    if let Some(probe) = probe(&outcome) {
        (outcome, wait) = settle(outcome, request, client, verse_id.as_deref(), probe).await;
    }
    if wait.is_none() && outcome.exit == ExitCode::Ok {
        // A write with no read-back probe is confirmed by its own reply; peers cannot be polled for it.
        wait = active_wait(request).map(|mode| match mode {
            WaitMode::Local => json!({"mode": "local", "settled": true, "ms": 0, "polls": 0, "basis": "write reply"}),
            WaitMode::Peers(_) => json!({"mode": "local", "settled": true, "ms": 0, "polls": 0, "basis": "write reply", "requested": "peers", "note": "no peer read-back exists for this verb; peers were not checked"}),
        });
    }
    let created_ids: Vec<String> = if created && !request.global.dry_run { created_id(outcome.data.as_ref()).into_iter().collect() } else { Vec::new() };
    let mut finished = result(request, context, outcome);
    finished.created_ids = created_ids;
    finished.wait = wait;
    finished.resolved_args = Some(resolved_args);
    finished
}
