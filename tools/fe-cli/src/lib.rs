//! fe-cli library: local-only operator CLI. See `tools/fe-cli/AGENTS.md`.

pub mod catalog;
pub mod commands;
pub mod context;
pub mod envelope;
pub mod help;
pub mod ids;
pub mod instance;
pub mod journal;
pub mod render;
pub mod schema_flags;
pub mod secrets;
pub mod transport;
pub mod x;

use std::collections::BTreeMap;
use std::io::Write;

/// Injected environment variables (never read globally).
pub type Env = BTreeMap<String, String>;

/// Everything one CLI run needs; tests build this in-process.
pub struct Invocation {
    /// Arguments after the program name.
    pub argv: Vec<String>,
    /// Environment snapshot.
    pub env: Env,
    /// Standard output sink.
    pub stdout: Box<dyn Write + Send>,
    /// Standard error sink.
    pub stderr: Box<dyn Write + Send>,
}

/// Runs one invocation and returns the process exit code (see `envelope::ExitCode`).
pub fn run(inv: Invocation) -> i32 {
    let Invocation { argv, env, mut stdout, mut stderr } = inv;
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(stderr, "fe: cannot start async runtime: {error}");
            return envelope::ExitCode::Internal.number();
        }
    };
    runtime.block_on(spine::execute(argv, env, &mut *stdout, &mut *stderr))
}

/// Boxed entry for `fe replay`, which runs each journal line as a child invocation (a boxed future breaks the async recursion).
pub(crate) fn execute_boxed<'a>(
    argv: Vec<String>,
    env: Env,
    stdout: &'a mut dyn Write,
    stderr: &'a mut dyn Write,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = i32> + 'a>> {
    Box::pin(spine::execute(argv, env, stdout, stderr))
}

/// The CLI spine: global flags, routing, instance + context resolution, dispatch, envelope, journal, exit code.
mod spine {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};

    use crate::catalog::{self, Door, Verb};
    use crate::commands::{self, CommandRequest, CommandResult, GlobalFlags};
    use crate::context::{self, Context};
    use crate::envelope::{self, CliError, Envelope, EnvelopeContext, ExitCode, OpInfo};
    use crate::instance;
    use crate::journal::{self, JournalInstance, JournalLine, ResolvedOp};
    use crate::schema_flags::{parse_flags, FlagKind, FlagSpec};
    use crate::transport::{self, Client, Outcome};
    use crate::{help, render, secrets, x, Env};

    const DEFAULT_WAIT_BUDGET_MS: u64 = 30_000;
    const MAX_RETRY_DELAY_MS: u64 = 10_000;
    /// Secret-bearing flag names whose values never reach the journal argv.
    const SECRET_FLAG_NEEDLES: &[&str] = &["token", "invite", "secret", "password", "ticket", "credential"];

    /// Global flags plus the remaining words, before routing.
    struct ParsedArgs {
        global: GlobalFlags,
        instance: Option<String>,
        help: bool,
        version: bool,
        words: Vec<String>,
    }

    /// Everything the final envelope and journal line are built from.
    struct Report {
        argv: Vec<String>,
        json: bool,
        started: Instant,
        seq: u64,
        op: OpInfo,
        resolved: ResolvedOp,
        instance: JournalInstance,
        ctx_before: Value,
        ctx_after: Value,
        context: EnvelopeContext,
        help: bool,
        note: Option<String>,
        human_text: Option<String>,
        created_ids: Vec<String>,
        wait: Option<Value>,
        /// Bearers held by the resolved instance, redacted from the journal wherever they appear.
        secrets: Vec<String>,
    }

    impl Report {
        fn new(argv: Vec<String>, started: Instant) -> Self {
            Self {
                argv,
                json: false,
                started,
                seq: 1,
                op: OpInfo::default(),
                resolved: ResolvedOp::default(),
                instance: JournalInstance::default(),
                ctx_before: Value::Null,
                ctx_after: Value::Null,
                context: EnvelopeContext::default(),
                help: false,
                note: None,
                human_text: None,
                created_ids: Vec::new(),
                wait: None,
                secrets: Vec::new(),
            }
        }

        fn describe(&mut self, verb: &str, via: &str, tool: Option<&str>, method: Option<&str>, path: Option<&str>) {
            self.op = OpInfo {
                verb: verb.to_string(),
                via: via.to_string(),
                tool: tool.map(str::to_string),
                method: method.map(str::to_string),
                path: path.map(str::to_string),
            };
            self.resolved = ResolvedOp {
                verb: verb.to_string(),
                via: via.to_string(),
                tool: tool.map(str::to_string),
                args: None,
                method: method.map(str::to_string),
                path: path.map(str::to_string),
            };
        }

        fn describe_row(&mut self, row: &Verb) {
            let name = catalog::path_of(row).join(" ");
            match row.door {
                Door::Mcp(tool) => self.describe(&name, "mcp", Some(tool), None, None),
                Door::Rest(method, path) => self.describe(&name, "rest", None, Some(method), Some(path)),
                Door::Local => self.describe(&name, "local", None, None, None),
            }
        }
    }

    /// Runs one invocation to completion and returns the exit code.
    pub async fn execute(argv: Vec<String>, env: Env, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
        let mut report = Report::new(argv.clone(), Instant::now());
        let parsed = match parse_globals(&argv) {
            Ok(parsed) => parsed,
            Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
        };
        report.json = parsed.global.json;
        if parsed.version {
            let _ = writeln!(stdout, "fe {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::Ok.number();
        }

        // One locked sidecar counter per agent (no journal scan); with a budget set, any counter failure fails closed.
        if let Some(journal_path) = non_empty(&env, "FE_JOURNAL").map(PathBuf::from) {
            let budget = non_empty(&env, "FE_BUDGET_MAX_COMMANDS").map(|text| text.trim().parse::<u64>());
            let max = match budget {
                Some(Ok(max)) => Some(max),
                Some(Err(_)) => {
                    let outcome = Outcome::failure(ExitCode::BudgetExhausted, journal::BUDGET_UNAVAILABLE_CODE, "FE_BUDGET_MAX_COMMANDS is not a count");
                    return conclude(report, outcome, &env, stdout, stderr);
                }
                None => None,
            };
            let counter = journal::counter_path(&journal_path, non_empty(&env, "FE_AGENT_ID"));
            match journal::charge(&counter, max) {
                Ok(charge) => {
                    report.seq = charge.seq;
                    if !charge.admitted {
                        let message = format!("command budget of {} exhausted for this agent", max.unwrap_or_default());
                        let outcome = Outcome::failure(ExitCode::BudgetExhausted, journal::BUDGET_EXHAUSTED_CODE, message);
                        return conclude(report, outcome, &env, stdout, stderr);
                    }
                }
                Err(error) if max.is_some() => {
                    let message = format!("the command budget counter is unavailable ({error}); refusing to run uncounted");
                    let outcome = Outcome::failure(ExitCode::BudgetExhausted, journal::BUDGET_UNAVAILABLE_CODE, message);
                    return conclude(report, outcome, &env, stdout, stderr);
                }
                Err(error) => {
                    let _ = writeln!(stderr, "fe: warning: journal sequence counter unavailable: {error}");
                }
            }
        }

        let words = parsed.words.clone();
        if words.is_empty() || words[0] == "help" {
            let path: Vec<String> = words.iter().skip(1).take_while(|word| !word.starts_with('-')).cloned().collect();
            return help_reply(report, &path, parsed.global.json, &env, stdout, stderr);
        }
        let noun = words[0].clone();
        if parsed.help && noun != "x" {
            let path: Vec<String> = words.iter().take_while(|word| !word.starts_with('-')).take(2).cloned().collect();
            return help_reply(report, &path, parsed.global.json, &env, stdout, stderr);
        }
        report.describe(&noun, "local", None, None, None);

        if let Some(allow) = non_empty(&env, "FE_CLI_ALLOW") {
            let allowed = noun == "note" || allow.split(',').any(|entry| entry.trim() == noun);
            if !allowed {
                let message = format!("'{noun}' is not allowed in this profile (FE_CLI_ALLOW)");
                return conclude(report, Outcome::failure(ExitCode::Refused, "not-allowed-in-profile", message), &env, stdout, stderr);
            }
        }

        // Routing is explicit and total: `x` takes every later token verbatim; everything else is one table row.
        let (verb, rest): (Option<&'static Verb>, Vec<String>) = if noun == "x" {
            (None, words[1..].to_vec())
        } else {
            match catalog::route(&words) {
                Ok((row, consumed)) => (Some(row), words[consumed..].to_vec()),
                Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
            }
        };
        let x_calling = verb.is_none()
            && rest.iter().take_while(|word| !word.starts_with('-')).count() >= 2
            && !rest.iter().any(|word| matches!(word.as_str(), "--help" | "-h" | "--list"));
        // Writes (and every `fe x` call) are resent by --retry only on a proven pre-admission refusal.
        let is_write = x_calling || verb.is_some_and(catalog::is_write);
        match verb {
            Some(row) => report.describe_row(row),
            None => {
                if x_calling {
                    report.describe("x", "rest", None, Some("POST"), Some("/petals/{petal_id}/extensions/{package_id}/commands/{command}"));
                } else {
                    report.describe("x", "rest", None, Some("GET"), Some("/petals/{petal_id}/extension-commands"));
                }
            }
        }

        let (args, positionals) = match verb {
            Some(row) => match split_args(row, &rest) {
                Ok(split) => split,
                Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
            },
            None => (BTreeMap::new(), rest.clone()),
        };
        let mut global = parsed.global.clone();
        apply_wait_default(&mut global, verb, &env);
        let dry_run = global.dry_run;
        report.resolved.args = serde_json::to_value(&args).ok().map(|value| journal::redact(&value));
        if noun == "note" {
            let text: String = positionals.join(" ").chars().take(2000).collect();
            report.note = Some(text);
        }
        let cwd = non_empty(&env, "FE_CWD")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let request = CommandRequest {
            noun: noun.clone(),
            verb,
            args,
            positionals,
            global,
            env: env.clone(),
            cwd: cwd.clone(),
            instance_flag: parsed.instance.clone(),
        };

        let mut result: CommandResult;
        let mut loaded = Context { v: 1, ..Context::default() };
        let mut context_file: Option<PathBuf> = None;
        if noun == "instance" || noun == "note" {
            result = run_local(&noun, &request, &env, &cwd);
        } else if noun == "replay" {
            // Replay resolves an instance per journal line (child invocations), never for itself.
            result = commands::replay::run(&request).await;
        } else {
            let selector = match instance_selector(&request, parsed.instance.as_deref(), &env) {
                Ok(selector) => selector,
                Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
            };
            let resolved = match instance::resolve(selector.as_deref(), &env, &cwd) {
                Ok(resolved) => resolved,
                Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
            };
            report.instance = JournalInstance { name: resolved.name.clone(), api_base: resolved.api_base.clone(), role: None };
            let client = match Client::new(resolved.clone()) {
                Ok(client) => client,
                Err(error) => return conclude(report, Outcome::from_error(&error), &env, stdout, stderr),
            };
            // One guard for every verb, before anything is sent, printed (--dry-run) or journaled verbatim.
            report.secrets = client.secrets();
            let carries_token = secrets::value_has_secret(&serde_json::to_value(&request.args).unwrap_or(Value::Null), &report.secrets)
                || rest.iter().any(|word| secrets::text_has_secret(word, &report.secrets));
            let token_file_named = secrets::refuse_token_file_args(&rest, &resolved.tokens_path);
            if carries_token || token_file_named.is_err() {
                return conclude(report, Outcome::from_error(&secrets::token_in_args()), &env, stdout, stderr);
            }
            let path = context::context_path(&env, &resolved.name);
            loaded = match context::load(&path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    let _ = writeln!(stderr, "fe: warning: {}", error.message);
                    Context { v: 1, ..Context::default() }
                }
            };
            context_file = Some(path);
            report.instance.role = role_for(&resolved, &loaded);
            let state_dir = context::state_dir(&env);
            result = invoke(&request, &client, &loaded, &state_dir).await;
            let mut attempts = 0;
            while transport::retry_allowed(&result.outcome, is_write) && attempts < request.global.retry {
                attempts += 1;
                let delay = result.outcome.retry_after_ms.unwrap_or(1000).min(MAX_RETRY_DELAY_MS);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                result = invoke(&request, &client, &loaded, &state_dir).await;
            }
        }

        if let Some(resolved_args) = &result.resolved_args {
            report.resolved.args = Some(resolved_args.clone());
        }
        report.ctx_before = serde_json::to_value(&loaded).unwrap_or(Value::Null);
        let mut context_after = loaded.clone();
        let mut outcome = result.outcome.clone();
        if let Some(after) = &result.context_after {
            context_after = after.clone();
            if context_after != loaded && !dry_run && outcome.exit == ExitCode::Ok {
                if let Some(path) = &context_file {
                    if let Err(error) = context::save(path, &context_after) {
                        outcome = Outcome::from_error(&error);
                    }
                }
            }
        }
        report.ctx_after = serde_json::to_value(&context_after).unwrap_or(Value::Null);
        report.context = merge_context(&result.resolved_context, &context_after, &report.instance.name);
        report.created_ids = result.created_ids;
        report.wait = annotate_wait(result.wait, request.global.wait_fallback.as_deref());
        conclude(report, outcome, &env, stdout, stderr)
    }

    /// `fe help [path]` and `<path> --help`: an unknown path exits 6 naming the nearest commands.
    fn help_reply(mut report: Report, path: &[String], json: bool, env: &Env, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
        report.describe("help", "local", None, None, None);
        report.help = true;
        let is_x = path.first().map(String::as_str) == Some("x");
        let rows: Vec<&'static Verb> = if path.is_empty() {
            catalog::verb_table().iter().filter(|row| row.phase <= catalog::SHIPPED_PHASE).collect()
        } else if is_x {
            Vec::new()
        } else {
            let whole_noun: Vec<&'static Verb> = catalog::verb_table().iter().filter(|row| row.noun == path[0]).collect();
            match catalog::route(path) {
                Ok((row, consumed)) if consumed == path.len() || row.verb.is_empty() => vec![row],
                Err(_) if path.len() == 1 && !whole_noun.is_empty() => whole_noun,
                Ok(_) | Err(_) => {
                    let error = catalog::route(&path[..path.len().min(2)]).err().unwrap_or_else(|| {
                        CliError::new(ExitCode::Usage, "unknown-command", format!("no help for `{}`; run `fe help`", path.join(" ")))
                    });
                    return conclude(report, Outcome::from_error(&error), env, stdout, stderr);
                }
            }
        };
        let mut outcome = Outcome::success(None, None);
        if json {
            outcome.data = Some(help::help_json_for(&rows));
        } else if is_x {
            report.human_text = Some(format!("{}\n{}", help::X_USAGE, help::GLOBAL_FLAG_RULE));
        } else {
            let shown: Vec<String> = if rows.len() == 1 { catalog::path_of(rows[0]).iter().map(|word| word.to_string()).collect() } else { path.to_vec() };
            report.human_text = Some(help::help_text(&shown));
        }
        conclude(report, outcome, env, stdout, stderr)
    }

    /// `--instance`, `FE_INSTANCE`, then the instance a per-agent `FE_CONTEXT` file selected (`fe use instance`).
    fn instance_selector(request: &CommandRequest, flag: Option<&str>, env: &Env) -> Result<Option<String>, CliError> {
        if request.verb.is_some_and(|row| (row.noun, row.verb) == ("use", "instance")) {
            let target = request.positionals.first().cloned().filter(|text| !text.trim().is_empty());
            return target
                .map(Some)
                .ok_or_else(|| CliError::new(ExitCode::Usage, "invalid-args", "usage: fe use instance <name|dir|tokens.json>"));
        }
        if let Some(flag) = flag {
            return Ok(Some(flag.to_string()));
        }
        if non_empty(env, "FE_INSTANCE").is_some() {
            return Ok(None);
        }
        let Some(path) = non_empty(env, "FE_CONTEXT") else { return Ok(None) };
        Ok(context::load(Path::new(path)).ok().and_then(|saved| saved.instance))
    }

    /// `FE_WAIT_DEFAULT` applies to writes only; an unsupported mode is downgraded to `local` and reported.
    fn apply_wait_default(global: &mut GlobalFlags, verb: Option<&'static Verb>, env: &Env) {
        if global.wait.is_some() || !verb.is_some_and(catalog::is_write) {
            return;
        }
        let Some(default) = non_empty(env, "FE_WAIT_DEFAULT").map(str::trim) else { return };
        match default {
            "none" | "off" => {}
            "local" => global.wait = Some("local".to_string()),
            other if commands::parse_wait(other).is_some() => {
                global.wait = Some(other.to_string());
                global.wait_default = true;
            }
            other => {
                global.wait = Some("local".to_string());
                global.wait_fallback = Some(other.to_string());
            }
        }
    }

    /// Adds `requested` + `note` to the envelope `wait` when an env default was downgraded.
    fn annotate_wait(wait: Option<Value>, fallback: Option<&str>) -> Option<Value> {
        let Some(requested) = fallback else { return wait };
        let mut wait = match wait {
            Some(Value::Object(map)) => map,
            _ => serde_json::Map::from_iter([("mode".to_string(), json!("local"))]),
        };
        wait.insert("requested".to_string(), json!(requested));
        wait.insert(
            "note".to_string(),
            json!(format!("FE_WAIT_DEFAULT={requested} is not a wait mode (local, peers, peers=<name>,<name>); waited with local")),
        );
        Some(Value::Object(wait))
    }

    /// Runs the verbs that need no network (`instance`, `note`).
    fn run_local(noun: &str, request: &CommandRequest, env: &Env, cwd: &Path) -> CommandResult {
        if noun == "instance" {
            commands::instance::run(request, env, cwd)
        } else {
            commands::note::run(request)
        }
    }

    /// Routes one request to `fe x` or the core verbs.
    async fn invoke(request: &CommandRequest, client: &Client, context: &Context, state_dir: &Path) -> CommandResult {
        if request.noun == "x" {
            x::run(request, client, context, state_dir).await
        } else {
            commands::dispatch(request, client, context).await
        }
    }

    /// Role of the token for the context's verse, else the first recorded role.
    fn role_for(resolved: &instance::Instance, context: &Context) -> Option<String> {
        let tokens = instance::read_tokens(resolved).ok()?;
        let by_verse = context.verse.as_ref().and_then(|verse| tokens.roles.get(&verse.id));
        by_verse.or_else(|| tokens.roles.values().next()).cloned()
    }

    /// Envelope context: the verb's own report, topped up from the saved context.
    fn merge_context(reported: &EnvelopeContext, saved: &Context, instance_name: &str) -> EnvelopeContext {
        let pick = |reported: &Option<String>, saved: &Option<context::NamedRef>| {
            reported.clone().or_else(|| saved.as_ref().map(|named| named.id.clone()))
        };
        EnvelopeContext {
            instance: reported.instance.clone().or_else(|| (!instance_name.is_empty()).then(|| instance_name.to_string())),
            verse_id: pick(&reported.verse_id, &saved.verse),
            fractal_id: pick(&reported.fractal_id, &saved.fractal),
            petal_id: pick(&reported.petal_id, &saved.petal),
        }
    }

    /// Prints the envelope, appends the journal line and returns the exit code.
    fn conclude(report: Report, outcome: Outcome, env: &Env, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
        let latency_ms = report.started.elapsed().as_millis() as u64;
        let envelope = Envelope {
            v: 1,
            ok: outcome.exit == ExitCode::Ok,
            exit: outcome.exit.number(),
            code: outcome.code.clone(),
            message: outcome.message.clone(),
            data: outcome.data.clone(),
            retry_after_ms: outcome.retry_after_ms,
            refusal: outcome.refusal.clone(),
            op: report.op.clone(),
            context: report.context.clone(),
            cid: ulid::Ulid::new().to_string(),
            latency_ms,
            wait: report.wait.clone(),
        };
        let line = envelope.to_json_line();
        if report.json {
            let _ = writeln!(stdout, "{line}");
        } else if let Some(text) = &report.human_text {
            let _ = writeln!(stdout, "{text}");
        } else {
            let text = render::render_human(&envelope);
            if envelope.ok {
                let _ = writeln!(stdout, "{text}");
            } else {
                let _ = writeln!(stderr, "{text}");
            }
        }

        if let Some(path) = non_empty(env, "FE_JOURNAL") {
            let now = envelope::unix_millis_now();
            let entry = JournalLine {
                v: 1,
                run: non_empty(env, "FE_RUN_ID").map(str::to_string),
                agent: non_empty(env, "FE_AGENT_ID").map(str::to_string),
                seq: report.seq,
                t: envelope::format_rfc3339(now),
                unix_ms: now,
                latency_ms,
                argv: redact_argv(&report.argv).iter().map(|word| journal::redact_word(word, &report.secrets)).collect(),
                resolved: ResolvedOp {
                    args: report.resolved.args.as_ref().map(|args| journal::redact_with(args, &report.secrets)),
                    ..report.resolved.clone()
                },
                ctx_before: report.ctx_before.clone(),
                ctx_after: report.ctx_after.clone(),
                instance: report.instance.clone(),
                exit: envelope.exit,
                code: envelope.code.clone(),
                envelope_digest: blake3::hash(line.as_bytes()).to_hex().to_string(),
                created_ids: report.created_ids.clone(),
                data_small: envelope.data.as_ref().map(|data| journal::redact_strings(&journal::data_small(data), &report.secrets, None)),
                wait: report.wait.clone(),
                help: report.help,
                note: report.note.clone(),
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
                catalog_digest: catalog::catalog_digest(),
            };
            if let Err(error) = journal::append_line(Path::new(path), &entry) {
                let _ = writeln!(stderr, "fe: warning: journal write failed: {error}");
            }
        }
        outcome.exit.number()
    }

    /// Blanks the value of any secret-named flag before argv is journaled.
    fn redact_argv(argv: &[String]) -> Vec<String> {
        let is_secret = |name: &str| {
            let lowered = name.to_ascii_lowercase();
            SECRET_FLAG_NEEDLES.iter().any(|needle| lowered.contains(needle))
        };
        let mut redacted = Vec::with_capacity(argv.len());
        let mut blank_next = false;
        for token in argv {
            if blank_next {
                redacted.push("<redacted>".to_string());
                blank_next = false;
            } else if let Some(flag) = token.strip_prefix("--") {
                match flag.split_once('=') {
                    Some((name, _)) if is_secret(name) => redacted.push(format!("--{name}=<redacted>")),
                    None if is_secret(flag) => {
                        redacted.push(token.clone());
                        blank_next = true;
                    }
                    _ => redacted.push(token.clone()),
                }
            } else {
                redacted.push(token.clone());
            }
        }
        redacted
    }

    fn non_empty<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
        env.get(key).map(String::as_str).filter(|value| !value.trim().is_empty())
    }

    /// Extracts global flags anywhere before a literal `--`, except after the `x` noun, whose tail is verbatim
    /// (only `--json` is noted there, and it stays in the tail).
    fn parse_globals(argv: &[String]) -> Result<ParsedArgs, CliError> {
        let mut parsed = ParsedArgs {
            global: GlobalFlags { wait_budget_ms: DEFAULT_WAIT_BUDGET_MS, ..GlobalFlags::default() },
            instance: None,
            help: false,
            version: false,
            words: Vec::new(),
        };
        let usage = |message: String| CliError::new(ExitCode::Usage, "invalid-usage", message);
        let mut index = 0;
        while index < argv.len() {
            if parsed.words.first().map(String::as_str) == Some("x") {
                for token in &argv[index..] {
                    if token == "--json" {
                        parsed.global.json = true;
                    }
                    parsed.words.push(token.clone());
                }
                break;
            }
            let token = &argv[index];
            index += 1;
            if token == "--" {
                parsed.words.push(token.clone());
                parsed.words.extend(argv[index..].iter().cloned());
                break;
            }
            if token == "-h" {
                parsed.help = true;
                continue;
            }
            let Some(flag) = token.strip_prefix("--") else {
                parsed.words.push(token.clone());
                continue;
            };
            let (name, inline) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (flag, None),
            };
            match name {
                "json" => parsed.global.json = true,
                "dry-run" => parsed.global.dry_run = true,
                "help" => parsed.help = true,
                "version" => parsed.version = true,
                "wait" => {
                    // `--wait` alone means local; it only swallows the next word when that word is a wait mode.
                    let next_is_mode = argv.get(index).is_some_and(|next| next == "local" || next.starts_with("peers"));
                    let value = match inline {
                        Some(value) => value,
                        None if next_is_mode => {
                            index += 1;
                            argv[index - 1].clone()
                        }
                        None => "local".to_string(),
                    };
                    parsed.global.wait = Some(value);
                }
                "instance" | "wait-budget" | "retry" | "verse" | "fractal" | "petal" => {
                    let value = match inline {
                        Some(value) => value,
                        None => match argv.get(index).filter(|next| !next.starts_with('-')) {
                            Some(next) => {
                                index += 1;
                                next.clone()
                            }
                            None => return Err(usage(format!("--{name} needs a value"))),
                        },
                    };
                    match name {
                        "instance" => parsed.instance = Some(value),
                        "wait-budget" => {
                            parsed.global.wait_budget_ms = parse_duration_ms(&value)
                                .ok_or_else(|| usage(format!("--wait-budget '{value}' is not a duration (e.g. 30s, 500ms)")))?;
                        }
                        "retry" => {
                            parsed.global.retry =
                                value.parse().map_err(|_| usage(format!("--retry '{value}' is not a count")))?;
                        }
                        "verse" => parsed.global.verse = Some(value),
                        "fractal" => parsed.global.fractal = Some(value),
                        _ => parsed.global.petal = Some(value),
                    }
                }
                _ => parsed.words.push(token.clone()),
            }
        }
        Ok(parsed)
    }

    /// `500ms`, `30s`, `2m`, or bare seconds.
    fn parse_duration_ms(text: &str) -> Option<u64> {
        let text = text.trim();
        let (digits, scale) = if let Some(digits) = text.strip_suffix("ms") {
            (digits, 1)
        } else if let Some(digits) = text.strip_suffix('s') {
            (digits, 1000)
        } else if let Some(digits) = text.strip_suffix('m') {
            (digits, 60_000)
        } else {
            (text, 1000)
        };
        digits.trim().parse::<u64>().ok().map(|amount| amount.saturating_mul(scale))
    }

    /// Splits a row's tokens into flag-parsed args (from `catalog::flag_specs`) and positional words.
    fn split_args(row: &Verb, rest: &[String]) -> Result<(BTreeMap<String, Value>, Vec<String>), CliError> {
        let specs = catalog::flag_specs(row);
        let (flag_tokens, positionals) = partition(&specs, rest);
        let parsed = parse_flags(&specs, &flag_tokens)?;
        Ok((parsed.into_iter().collect(), positionals))
    }

    /// Separates `--flag [value]` tokens from bare positional words using the specs' value kinds.
    fn partition(specs: &[FlagSpec], rest: &[String]) -> (Vec<String>, Vec<String>) {
        let mut flags = Vec::new();
        let mut positionals = Vec::new();
        let mut index = 0;
        while index < rest.len() {
            let token = &rest[index];
            index += 1;
            if token == "--" {
                positionals.extend(rest[index..].iter().cloned());
                break;
            }
            let Some(name) = token.strip_prefix("--") else {
                positionals.push(token.clone());
                continue;
            };
            flags.push(token.clone());
            if name.contains('=') {
                continue;
            }
            let takes_value = match name {
                "args" | "args-json" => true,
                _ => {
                    let known = specs.iter().find(|spec| spec.flag == name);
                    let negated = name.strip_prefix("no-").and_then(|bare| specs.iter().find(|spec| spec.flag == bare));
                    match (known, negated) {
                        (Some(spec), _) => spec.kind != FlagKind::Boolean || spec.repeatable,
                        (None, Some(_)) => false,
                        (None, None) => false,
                    }
                }
            };
            if takes_value {
                if let Some(value) = rest.get(index) {
                    flags.push(value.clone());
                    index += 1;
                }
            }
        }
        (flags, positionals)
    }
}
