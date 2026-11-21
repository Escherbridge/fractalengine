//! `fe help [--json]` generation (S3).

use serde_json::{json, Value};

use crate::catalog::{self, Door, Verb};
use crate::envelope::ExitCode;
use crate::schema_flags::{FlagKind, FlagSpec};

/// How global flags combine with command words (one rule, shown in both help forms).
pub const GLOBAL_FLAG_RULE: &str = "Global flags (--json --dry-run --wait local --wait-budget <dur> --retry <n> --instance <name|dir|tokens.json> --verse|--fractal|--petal <id>) may come before or after the command words. After `fe x` every token belongs to the extension command (only --json, --help/-h and --list are read there), so put other global flags before `x`.";

fn kind_name(kind: FlagKind) -> &'static str {
    match kind {
        FlagKind::String => "string",
        FlagKind::Integer => "integer",
        FlagKind::Number => "number",
        FlagKind::Boolean => "boolean",
        FlagKind::Json => "json",
    }
}

fn arg_json(spec: &FlagSpec, positional: Option<usize>) -> Value {
    json!({
        "name": spec.name,
        "flag": spec.flag,
        "type": kind_name(spec.kind),
        "required": spec.required,
        "enum": spec.enum_values,
        "description": spec.description,
        "from_context": spec.from_context,
        "repeatable": spec.repeatable,
        "positional": positional,
    })
}

/// Arguments of a verb: generated flags plus bare-word arguments of local verbs.
fn verb_args(verb: &Verb) -> Vec<Value> {
    let detail = catalog::detail(verb);
    let mut args: Vec<Value> = catalog::flag_specs(verb)
        .iter()
        .map(|spec| arg_json(spec, detail.positionals.iter().position(|name| *name == spec.name)))
        .collect();
    if verb.noun == "use" {
        args.push(json!({"name": "target", "flag": null, "type": "string", "required": true, "enum": [], "description": "Instance name, or an id or name at that level", "from_context": null, "repeatable": false, "positional": 0}));
    }
    if verb.noun == "note" {
        args.push(json!({"name": "text", "flag": null, "type": "string", "required": true, "enum": [], "description": "What was confusing or missing", "from_context": null, "repeatable": false, "positional": 0}));
    }
    args
}

fn command_json(verb: &Verb) -> Value {
    let path = catalog::path_of(verb);
    let (via, tool, method, route) = match verb.door {
        Door::Mcp(tool) => ("mcp", Some(tool), None, None),
        Door::Rest(method, route) => ("rest", None, Some(method), Some(route)),
        Door::Local => ("local", None, None, None),
    };
    let networked = verb.door != Door::Local;
    let wait: Vec<&str> = if catalog::is_write(verb) { vec!["local"] } else { Vec::new() };
    json!({
        "path": path,
        "summary": verb.summary,
        "via": via,
        "tool": tool,
        "method": method,
        "route": route,
        "min_role": catalog::min_role(verb),
        "scope": catalog::verb_scope(verb),
        "args": verb_args(verb),
        "args_json_only": catalog::args_json_only(verb),
        "supports": {
            "json": true,
            "dry_run": networked,
            "wait": wait,
        },
    })
}

fn exit_codes() -> Value {
    let table = [
        (ExitCode::Ok, "ok", "success"),
        (ExitCode::Internal, "internal", "CLI internal error"),
        (ExitCode::Refused, "refused", "refused by policy, hook or extension"),
        (ExitCode::Transport, "transport", "not running, non-loopback, token file missing or expired"),
        (ExitCode::RoleDenied, "role-denied", "role too low for this tool"),
        (ExitCode::NotFound, "not-found", "not found or not permitted (uniform by design)"),
        (ExitCode::Usage, "usage", "invalid input or usage"),
        (ExitCode::Retryable, "retryable", "pre-admission refusal; retry after retry_after_ms"),
        (ExitCode::UnknownOutcome, "unknown-outcome", "outcome unknown; reconcile by reading, never retried"),
        (ExitCode::WaitExpired, "wait-expired", "write accepted but --wait budget expired"),
        (ExitCode::ServerFault, "server-fault", "server fault or unexpected reply"),
        (ExitCode::BudgetExhausted, "budget-exhausted", "command budget exhausted"),
    ];
    Value::Array(table.iter().map(|(code, name, meaning)| json!({"code": code.number(), "name": name, "meaning": meaning})).collect())
}

/// Machine-readable help document (plan §1 "`fe help --json`").
pub fn help_json() -> Value {
    help_json_for(&catalog::verb_table().iter().filter(|verb| verb.phase == 1).collect::<Vec<_>>())
}

/// The help document restricted to `rows`.
pub fn help_json_for(rows: &[&Verb]) -> Value {
    json!({
        "v": 1,
        "cli_version": env!("CARGO_PKG_VERSION"),
        "catalog_digest": catalog::catalog_digest(),
        "exit_codes": exit_codes(),
        "envelope_schema": {
            "v": 1,
            "fields": ["v", "ok", "exit", "code", "message", "data", "retry_after_ms", "refusal", "op", "context", "cid", "latency_ms", "wait"],
        },
        "global_flags": { "names": catalog::GLOBAL_FLAGS, "rule": GLOBAL_FLAG_RULE },
        "commands": rows.iter().map(|verb| command_json(verb)).collect::<Vec<_>>(),
        "dynamic": { "x": "fe x --list --json", "usage": X_USAGE },
    })
}

fn usage_line(verb: &Verb) -> String {
    let detail = catalog::detail(verb);
    let mut line = format!("fe {}", verb.noun);
    if !verb.verb.is_empty() {
        line.push(' ');
        line.push_str(verb.verb);
    }
    for positional in detail.positionals {
        line.push_str(&format!(" <{positional}>"));
    }
    line
}

/// Human help for the whole tree, or one `noun [verb]` path.
pub fn help_text(path: &[String]) -> String {
    let noun = path.first().map(String::as_str);
    let verb_name = path.get(1).map(String::as_str);
    let matching: Vec<&Verb> = catalog::verb_table()
        .iter()
        .filter(|verb| verb.phase == 1)
        .filter(|verb| noun.is_none_or(|wanted| verb.noun == wanted))
        .filter(|verb| verb_name.is_none_or(|wanted| verb.verb == wanted))
        .collect();
    if matching.is_empty() {
        return format!("no help for `{}`; run `fe help` for the command list", path.join(" "));
    }
    let detailed = verb_name.is_some() || matching.len() == 1;
    let mut text = String::new();
    if noun.is_none() {
        text.push_str("fe: local FractalEngine CLI (loopback only).\n");
        text.push_str(GLOBAL_FLAG_RULE);
        text.push_str("\n\n");
    }
    for verb in matching {
        text.push_str(&format!("{:<34} {}\n", usage_line(verb), verb.summary));
        if detailed {
            for spec in catalog::flag_specs(verb) {
                let note = match &spec.from_context {
                    Some(field) => format!(" (default: current {field})"),
                    None => String::new(),
                };
                text.push_str(&format!("    --{:<18} {}{}\n", spec.flag, spec.description, note));
            }
            let hidden = catalog::args_json_only(verb);
            if !hidden.is_empty() {
                text.push_str(&format!("    --args-json only: {}\n", hidden.join(", ")));
            }
            if let Some(role) = catalog::min_role(verb) {
                text.push_str(&format!("    minimum role: {role}\n"));
            }
        }
    }
    if noun.is_none() {
        text.push_str("\nExtension commands: ");
        text.push_str(X_USAGE);
        text.push_str(". Machine catalog: fe help --json.\n");
    }
    text
}

/// Usage line of the dynamic `fe x` noun.
pub const X_USAGE: &str = "fe x [<package> [<command>]] [command flags] (--list, --help, --json)";
