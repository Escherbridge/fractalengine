//! `fe x`: extension-contributed commands via the discovery route (S4).
//!
//! Discovery text is untrusted data: it is only sent back as data or rendered through `terminal_safe`.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::commands::{CommandRequest, CommandResult};
use crate::context::Context;
use crate::envelope::{CliError, EnvelopeContext, ExitCode};
use crate::instance::{self, TokenFile};
use crate::render::terminal_safe;
use crate::schema_flags::{args_from_schema, args_json_only, flag_help, parse_flags};
use crate::transport::{Client, Outcome};
use crate::{ids, secrets};

/// Handles `fe x [<pkg> [<cmd>]]` (list, help, call); `request.positionals` holds every word after `x`.
pub async fn run(request: &CommandRequest, client: &Client, context: &Context, state_dir: &Path) -> CommandResult {
    let petal_id = request.global.petal.clone().or_else(|| context.petal.as_ref().map(|petal| petal.id.clone()));
    let verse_id = request.global.verse.clone().or_else(|| context.verse.as_ref().map(|verse| verse.id.clone()));
    let resolved_context = EnvelopeContext {
        instance: context.instance.clone(),
        verse_id: verse_id.clone(),
        fractal_id: context.fractal.as_ref().map(|fractal| fractal.id.clone()),
        petal_id: petal_id.clone(),
    };
    let outcome = match execute(request, client, state_dir, petal_id.as_deref(), verse_id.as_deref()).await {
        Ok(outcome) => outcome,
        Err(error) => failure(error),
    };
    CommandResult { outcome, context_after: None, created_ids: Vec::new(), resolved_context, wait: None, resolved_args: None }
}

/// Words and switches of one `fe x` invocation.
#[derive(Debug, Default, PartialEq)]
struct XArguments {
    package: Option<String>,
    command: Option<String>,
    flags: Vec<String>,
    help: bool,
    list: bool,
}

/// Splits the raw words: switches removed, then up to two leading words name package and command.
fn split_arguments(tokens: &[String]) -> XArguments {
    let mut arguments = XArguments::default();
    let mut rest: Vec<&String> = Vec::new();
    for token in tokens {
        match token.as_str() {
            "--help" | "-h" => arguments.help = true,
            "--list" => arguments.list = true,
            "--json" => {}
            _ => rest.push(token),
        }
    }
    let mut words = rest.iter().take_while(|token| !token.starts_with('-')).take(2).map(|token| token.to_string());
    arguments.package = words.next();
    arguments.command = words.next();
    let consumed = usize::from(arguments.package.is_some()) + usize::from(arguments.command.is_some());
    arguments.flags = rest.into_iter().skip(consumed).cloned().collect();
    arguments
}

/// Result of naming a package by id or unique id suffix.
#[derive(Debug, PartialEq)]
enum PackageMatch {
    Found(usize),
    Ambiguous(Vec<usize>),
    Missing,
}

/// An exact id wins; otherwise a unique id suffix matches.
fn match_package(ids: &[&str], wanted: &str) -> PackageMatch {
    if let Some(index) = ids.iter().position(|id| *id == wanted) {
        return PackageMatch::Found(index);
    }
    let matches: Vec<usize> = ids.iter().enumerate().filter(|(_, id)| id.ends_with(wanted)).map(|(index, _)| index).collect();
    match matches.as_slice() {
        [] => PackageMatch::Missing,
        [only] => PackageMatch::Found(*only),
        _ => PackageMatch::Ambiguous(matches),
    }
}

async fn execute(
    request: &CommandRequest,
    client: &Client,
    state_dir: &Path,
    petal_id: Option<&str>,
    verse_id: Option<&str>,
) -> Result<Outcome, CliError> {
    let arguments = split_arguments(&request.positionals);
    let Some(petal_id) = petal_id else {
        return Err(CliError::new(ExitCode::Usage, "no-petal", "no petal selected; run `fe use petal <name>` or pass --petal"));
    };
    ids::require_route_id("--petal", petal_id)?;
    if let Some(verse_id) = verse_id {
        ids::require_route_id("--verse", verse_id)?;
    }
    if arguments.command.is_none() && !arguments.flags.is_empty() {
        return Err(CliError::new(ExitCode::Usage, "invalid-input", format!("{} needs a command to apply to", arguments.flags[0])));
    }
    let calling = arguments.command.is_some() && !arguments.help && !arguments.list;

    // Keyed by who asked: a less-privileged context never sees a catalog cached for a stronger token.
    let role = instance::read_tokens(&client.instance).ok().and_then(|tokens| picked_role(&tokens, verse_id));
    let cache_path = cache_file(state_dir, &client.instance.api_base, petal_id, verse_id, role.as_deref());
    let fetched = client.rest("GET", &format!("/petals/{petal_id}/extension-commands"), None, verse_id).await;
    let (document, stale) = if fetched.exit == ExitCode::Ok {
        let Some(document) = extract_document(fetched.data.as_ref()) else {
            return Err(CliError::new(ExitCode::ServerFault, "bad-discovery-reply", "the discovery route returned no package list"));
        };
        write_cache(&cache_path, &document);
        (document, false)
    } else if fetched.code == "not-running" && !calling && role.is_some() {
        // Only an unreachable instance serves the cache; a rejected, expired or missing token never does.
        match read_cache(&cache_path) {
            Some(document) => (document, true),
            None => return Ok(fetched),
        }
    } else {
        return Ok(fetched);
    };

    let packages = document.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(wanted) = arguments.package.as_deref() else {
        let mut data = document.as_object().cloned().unwrap_or_default();
        data.insert("stale".into(), Value::Bool(stale));
        if arguments.help {
            data.insert("help".into(), Value::String("fe x [<package> [<command>]] [flags]  (--list --json for the raw catalog)".into()));
        }
        return Ok(success(Value::Object(data)));
    };

    let ids: Vec<&str> = packages.iter().map(|package| text(package, "package_id")).collect();
    let package = match match_package(&ids, wanted) {
        PackageMatch::Found(index) => &packages[index],
        PackageMatch::Ambiguous(indexes) => {
            let names: Vec<&str> = indexes.iter().map(|index| ids[*index]).collect();
            return Err(CliError::new(ExitCode::Usage, "ambiguous-package", format!("{wanted} matches several packages: {}", names.join(", "))));
        }
        PackageMatch::Missing => {
            return Err(CliError::new(ExitCode::NotFound, "package-not-found", format!("no extension package matching {wanted} at this petal")));
        }
    };
    let package_id = text(package, "package_id");

    let Some(command_id) = arguments.command.as_deref() else {
        let mut data = document_header(&document, stale);
        data.insert("package".into(), package.clone());
        if arguments.help {
            data.insert("help".into(), Value::String(format!("fe x {} <command> [flags]", terminal_safe(package_id))));
        }
        return Ok(success(Value::Object(data)));
    };
    let commands = package.get("commands").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(command) = commands.iter().find(|command| text(command, "id") == command_id) else {
        let known: Vec<&str> = commands.iter().map(|command| text(command, "id")).collect();
        return Err(CliError::new(
            ExitCode::NotFound,
            "command-not-found",
            format!("{package_id} has no command {command_id}; available: {}", known.join(", ")),
        ));
    };
    let schema = command.get("args_schema").cloned().unwrap_or(Value::Null);

    if !calling {
        let mut data = document_header(&document, stale);
        data.insert("package_id".into(), json!(package_id));
        data.insert("command".into(), command.clone());
        data.insert("help".into(), Value::String(command_help(package_id, command, &schema)));
        return Ok(success(Value::Object(data)));
    }

    // Colliding or unsafe property names stay reachable through --args-json (listed by `--help`).
    let specs = args_from_schema(&schema);
    let args = Value::Object(parse_flags(&specs, &arguments.flags)?);
    // `--args @file` content is only known here; the spine checked the raw words.
    if secrets::value_has_secret(&args, &client.secrets()) {
        return Err(secrets::token_in_args());
    }
    // Ids from discovery are host data too: validated before they enter a route.
    ids::require_package_id(package_id)?;
    ids::require_route_id("command", command_id)?;
    if request.global.dry_run {
        let path = format!("/api/v1/petals/{petal_id}/extensions/{package_id}/commands/{command_id}");
        return Ok(success(json!({"dry_run": true, "method": "POST", "path": path, "body": {"args": args}})));
    }
    let mut outcome = client.extension_command(petal_id, package_id, command_id, args, verse_id).await;
    // A refusal from the extension or its role floor carries the host text verbatim (set by `classify_extension`).
    if outcome.refusal.reason.is_some() && outcome.refusal.package_id.is_none() {
        outcome.refusal.package_id = Some(package_id.to_string());
    }
    Ok(outcome)
}

/// Role of the token the request will carry (the verse's own, else the highest-ranked one), as `transport` picks it.
fn picked_role(tokens: &TokenFile, verse_id: Option<&str>) -> Option<String> {
    match verse_id {
        Some(verse_id) => tokens.roles.get(verse_id).cloned(),
        None => tokens
            .tokens
            .keys()
            .filter_map(|verse| tokens.roles.get(verse))
            .max_by_key(|role| crate::transport::role_rank(Some(role.as_str())))
            .cloned(),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn success(data: Value) -> Outcome {
    let mut outcome = Outcome::success(Some(data), None);
    outcome.message = String::new();
    outcome
}

fn failure(error: CliError) -> Outcome {
    Outcome::from_error(&error)
}

/// `petal_id`, `catalog_digest` and `stale` of a discovery document.
fn document_header(document: &Value, stale: bool) -> Map<String, Value> {
    let mut header = Map::new();
    for key in ["petal_id", "catalog_digest"] {
        if let Some(value) = document.get(key) {
            header.insert(key.into(), value.clone());
        }
    }
    header.insert("stale".into(), Value::Bool(stale));
    header
}

/// The discovery document, whether the client hands back the body or its `data` member.
fn extract_document(data: Option<&Value>) -> Option<Value> {
    let data = data?;
    if data.get("packages").is_some_and(Value::is_array) {
        return Some(data.clone());
    }
    data.get("data").filter(|inner| inner.get("packages").is_some_and(Value::is_array)).cloned()
}

/// Help text for one command; every extension-supplied string is single-lined by `terminal_safe` first, so a
/// description can never forge a help line.
fn command_help(package_id: &str, command: &Value, schema: &Value) -> String {
    let rate = command.get("rate_hz").and_then(Value::as_f64).map(|hz| format!("{hz} Hz")).unwrap_or_else(|| "unlimited".into());
    let yes_no = |key: &str| if command.get(key).and_then(Value::as_bool) == Some(true) { "yes" } else { "no" };
    let specs = args_from_schema(schema);
    let mut flags = flag_help(&specs);
    let hidden: Vec<String> = args_json_only(schema, &specs).iter().map(|name| terminal_safe(name)).collect();
    if !hidden.is_empty() {
        flags.push_str(&format!("\n  args-json only (no flag spelling): {}", hidden.join(", ")));
    }
    format!(
        "fe x {package} {id} [flags]\n{description}\nrole floor: {role}; callable: {callable}; rate: {rate}; external: {external}\nflags:\n{flags}",
        package = terminal_safe(package_id),
        id = terminal_safe(text(command, "id")),
        description = terminal_safe(text(command, "description")),
        role = terminal_safe(text(command, "role_floor")),
        callable = yes_no("callable"),
        external = yes_no("external"),
    )
}

/// `<state_dir>/x-cache/<digest of api_base, petal, verse, role>.json`; no id ever names a path segment.
fn cache_file(state_dir: &Path, api_base: &str, petal_id: &str, verse_id: Option<&str>, role: Option<&str>) -> PathBuf {
    let key = format!("{api_base}\u{0}{petal_id}\u{0}{}\u{0}{}", verse_id.unwrap_or(""), role.unwrap_or("").to_ascii_lowercase());
    let digest = blake3::hash(key.as_bytes()).to_hex();
    state_dir.join("x-cache").join(format!("{}.json", &digest.as_str()[..32]))
}

fn read_cache(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let stored: Value = serde_json::from_str(&text).ok()?;
    stored.get("document").filter(|document| document.get("packages").is_some_and(Value::is_array)).cloned()
}

/// Best-effort atomic write keyed by `catalog_digest`; skipped when the stored digest already matches.
fn write_cache(path: &Path, document: &Value) {
    let digest = document.get("catalog_digest").and_then(Value::as_str);
    if digest.is_some() && read_cache(path).as_ref().and_then(|stored| stored.get("catalog_digest").and_then(Value::as_str).map(str::to_string)).as_deref() == digest {
        return;
    }
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let stored = json!({"v": 1, "catalog_digest": digest, "document": document});
    let temporary = path.with_extension(format!("json.tmp{}", std::process::id()));
    let written = serde_json::to_vec(&stored).ok().is_some_and(|bytes| std::fs::write(&temporary, bytes).is_ok());
    if !written || std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn package_suffix_resolution() {
        let ids = ["org.example.fleet-sim", "org.example.fleet-viz", "com.other.fleet-sim", "short"];
        let cases: Vec<(&str, PackageMatch)> = vec![
            ("org.example.fleet-sim", PackageMatch::Found(0)),
            ("fleet-viz", PackageMatch::Found(1)),
            ("short", PackageMatch::Found(3)),
            ("fleet-sim", PackageMatch::Ambiguous(vec![0, 2])),
            ("nothing", PackageMatch::Missing),
        ];
        for (wanted, expected) in cases {
            assert_eq!(match_package(&ids, wanted), expected, "{wanted}");
        }
        // An exact id wins even when it is also a suffix of a longer id.
        assert_eq!(match_package(&["a.sim", "sim"], "sim"), PackageMatch::Found(1));
    }

    #[test]
    fn arguments_split_into_words_and_flags() {
        let parsed = split_arguments(&words(&["--list", "fleet", "spawn", "--count", "3", "--help"]));
        assert_eq!(
            parsed,
            XArguments {
                package: Some("fleet".into()),
                command: Some("spawn".into()),
                flags: words(&["--count", "3"]),
                help: true,
                list: true,
            }
        );
        assert_eq!(split_arguments(&words(&[])), XArguments::default());
    }
}
