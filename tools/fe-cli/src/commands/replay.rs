//! `fe replay <journal>`: re-run a recorded journal with no LLM in the loop (see AGENTS.md §replay).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::scenario::{self, kebab_name};
use super::{created_id, make_outcome, ok_outcome, usage_outcome, CommandRequest, CommandResult};
use crate::envelope::{EnvelopeContext, ExitCode};
use crate::journal::{self, JournalLine};
use crate::transport::Outcome;

/// Shortest id the replay rewrites inside argv words (shorter strings are names, not ids).
const MIN_REWRITTEN_ID_CHARS: usize = 8;

/// The journal agent that wrote a line: `FE_AGENT_ID`, else the instance name.
pub(super) fn agent_key(line: &JournalLine) -> String {
    line.agent.clone().filter(|agent| !agent.is_empty()).unwrap_or_else(|| line.instance.name.clone())
}

/// Why a line is not replayed (context, local and pre-dispatch lines change no instance state).
pub(super) fn skip_reason(line: &JournalLine) -> Option<&'static str> {
    if line.help {
        Some("help")
    } else if line.resolved.verb.is_empty() {
        Some("refused before dispatch")
    } else if line.resolved.via == "local" {
        Some("local verb")
    } else if line.resolved.verb.starts_with("use ") {
        Some("context verb")
    } else {
        None
    }
}

/// Runs `fe replay`: replays, or with `--emit-scenario` converts the journal.
pub async fn run(request: &CommandRequest) -> CommandResult {
    CommandResult {
        outcome: replay(request).await,
        context_after: None,
        created_ids: Vec::new(),
        resolved_context: EnvelopeContext::default(),
        wait: None,
        resolved_args: None,
    }
}

fn resolve_path(request: &CommandRequest, text: &str) -> PathBuf {
    let given = PathBuf::from(text);
    if given.is_absolute() { given } else { request.cwd.join(given) }
}

fn text_arg(request: &CommandRequest, name: &str) -> Option<String> {
    request.args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// `a=b` pairs of a repeatable flag.
fn pairs(request: &CommandRequest, name: &str) -> Result<Vec<(String, String)>, Outcome> {
    let mut found = Vec::new();
    for item in request.args.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        match item.split_once('=') {
            Some((left, right)) if !left.is_empty() && !right.is_empty() => found.push((left.to_string(), right.to_string())),
            _ => return Err(usage_outcome("invalid-input", format!("--{} expects old=new, got '{item}'", name.replace('_', "-")))),
        }
    }
    Ok(found)
}

fn read_journal(path: &Path) -> Result<Vec<JournalLine>, Outcome> {
    let text = std::fs::read_to_string(path).map_err(|error| usage_outcome("invalid-input", format!("cannot read journal {}: {}", path.display(), error.kind())))?;
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate().filter(|(_, raw)| !raw.trim().is_empty()) {
        let line = serde_json::from_str::<JournalLine>(raw.trim_start_matches('\u{feff}'))
            .map_err(|error| usage_outcome("invalid-journal", format!("{} line {} is not a journal line: {error}", path.display(), index + 1)))?;
        lines.push(line);
    }
    if lines.is_empty() {
        return Err(usage_outcome("invalid-journal", format!("{} holds no journal lines", path.display())));
    }
    Ok(lines)
}

async fn replay(request: &CommandRequest) -> Outcome {
    let Some(journal_arg) = request.positionals.first() else {
        return usage_outcome("missing-required", "usage: fe replay <journal.jsonl> [--map agent=instance] [--bind old=new] [--strict] [--emit-scenario out.json]");
    };
    let journal_path = resolve_path(request, journal_arg);
    let lines = match read_journal(&journal_path) {
        Ok(lines) => lines,
        Err(outcome) => return outcome,
    };
    let source = journal_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(target) = text_arg(request, "emit_scenario") {
        return emit_scenario(request, &lines, &source, &target);
    }
    let (maps, binds) = match (pairs(request, "map"), pairs(request, "bind")) {
        (Ok(maps), Ok(binds)) => (maps.into_iter().collect::<BTreeMap<_, _>>(), binds),
        (Err(outcome), _) | (_, Err(outcome)) => return outcome,
    };
    let strict = request.args.get("strict").and_then(Value::as_bool).unwrap_or(false);

    if request.global.dry_run {
        let steps: Vec<Value> = lines
            .iter()
            .filter(|line| skip_reason(line).is_none())
            .map(|line| json!({"seq": line.seq, "agent": agent_key(line), "verb": line.resolved.verb, "argv": line.argv, "recorded_exit": line.exit}))
            .collect();
        let mut outcome = ok_outcome(json!({"dry_run": true, "would_replay": steps, "journal_lines": lines.len()}));
        outcome.message = "dry run: nothing sent".to_string();
        return outcome;
    }

    let mut id_map: BTreeMap<String, String> = binds.into_iter().collect();
    let state_root = crate::context::state_dir(&request.env).join(format!("replay-{}", ulid::Ulid::new()));
    let mut context_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let (mut replayed, mut skipped) = (0usize, 0usize);
    let mut divergence: Option<(String, Value)> = None;
    for line in &lines {
        if skip_reason(line).is_some() {
            skipped += 1;
            continue;
        }
        if let Some(word) = line.argv.iter().find(|word| word.contains("<redacted")) {
            let _ = std::fs::remove_dir_all(&state_root);
            let message = format!("step {} ({}) was journaled with a redacted argument ({}…); it cannot be replayed", line.seq, line.resolved.verb, word.chars().take(12).collect::<String>());
            return usage_outcome("replay-unbindable", message);
        }
        let agent = agent_key(line);
        let next_index = context_files.len();
        let context_file = context_files.entry(agent.clone()).or_insert_with(|| state_root.join(format!("ctx-{next_index}.json"))).clone();
        let selector = maps.get(&agent).or_else(|| maps.get(&line.instance.name)).cloned();
        let (exit, envelope) = run_step(request, line, &id_map, &context_file, &agent, selector.as_deref()).await;
        replayed += 1;
        if exit != line.exit {
            let message = format!(
                "step {} ({}) exited {} but the journal recorded {}: {}",
                line.seq,
                line.resolved.verb,
                exit,
                line.exit,
                envelope.get("message").and_then(Value::as_str).unwrap_or("")
            );
            let detail = json!({
                "seq": line.seq, "agent": agent, "verb": line.resolved.verb,
                "recorded_exit": line.exit, "recorded_code": line.code,
                "replayed_exit": exit, "replayed_code": envelope.get("code"),
            });
            divergence = Some((message, detail));
            break;
        }
        if exit == 0 {
            if let (Some(original), Some(new)) = (line.created_ids.first(), created_id(envelope.get("data"))) {
                id_map.insert(original.clone(), new);
            }
        }
        if strict {
            if let Some(difference) = data_difference(line, &envelope, &id_map) {
                let detail = json!({"seq": line.seq, "agent": agent, "verb": line.resolved.verb, "recorded_exit": line.exit, "replayed_exit": exit, "data": difference});
                divergence = Some((format!("step {} ({}) answered different data than the journal under --strict", line.seq, line.resolved.verb), detail));
                break;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&state_root);

    let summary = json!({
        "journal": source,
        "journal_lines": lines.len(),
        "replayed": replayed,
        "skipped": skipped,
        "id_map": id_map,
        "diverged_at": divergence.as_ref().map(|(_, detail)| detail.clone()),
    });
    match divergence {
        Some((message, _)) => {
            let mut outcome = make_outcome(ExitCode::ReplayDiverged, "replay-diverged", message, Some(summary));
            outcome.refusal = Default::default();
            outcome
        }
        None => {
            let mut outcome = ok_outcome(summary);
            outcome.message = format!("replayed {replayed} steps; every exit code matches the journal");
            outcome
        }
    }
}

/// `--emit-scenario`: writes the scenario file and reports what became of every line.
fn emit_scenario(request: &CommandRequest, lines: &[JournalLine], source: &str, target: &str) -> Outcome {
    let name = text_arg(request, "scenario_name").unwrap_or_else(|| kebab_name(source.trim_end_matches(".jsonl")));
    let (document, mut report) = match scenario::emit(lines, &name, source) {
        Ok(emitted) => emitted,
        Err(error) => return make_outcome(error.exit, &error.code, error.message, None),
    };
    let path = resolve_path(request, target);
    let mut text = serde_json::to_string_pretty(&document).unwrap_or_default();
    text.push('\n');
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return usage_outcome("invalid-input", format!("cannot create {}: {}", parent.display(), error.kind()));
        }
    }
    if let Err(error) = std::fs::write(&path, text) {
        return usage_outcome("invalid-input", format!("cannot write {}: {}", path.display(), error.kind()));
    }
    report["scenario"] = json!(target);
    report["name"] = json!(name);
    let mut outcome = ok_outcome(report);
    outcome.message = "scenario written; check it with `python tools/qa/sim/run.py validate <path>`".to_string();
    outcome
}

/// Runs one journal line through the spine as a child invocation and returns its exit code and envelope.
async fn run_step(request: &CommandRequest, line: &JournalLine, id_map: &BTreeMap<String, String>, context_file: &Path, agent: &str, selector: Option<&str>) -> (i32, Value) {
    let mut argv: Vec<String> = vec!["--json".to_string()];
    if selector.is_none() {
        if let Some(flag) = &request.instance_flag {
            argv.push("--instance".to_string());
            argv.push(flag.clone());
        }
    }
    // Explicit scope ids make the step independent of any context file.
    for (flag, level) in [("--verse", "verse"), ("--fractal", "fractal"), ("--petal", "petal")] {
        if let Some(id) = line.ctx_before.get(level).and_then(|named| named.get("id")).and_then(Value::as_str) {
            argv.push(flag.to_string());
            argv.push(rewrite_ids(id, id_map));
        }
    }
    argv.extend(line.argv.iter().map(|word| rewrite_ids(word, id_map)));

    let mut env = request.env.clone();
    env.insert("FE_CONTEXT".to_string(), context_file.to_string_lossy().into_owned());
    env.insert("FE_AGENT_ID".to_string(), agent.to_string());
    if let Some(selector) = selector {
        env.insert("FE_INSTANCE".to_string(), selector.to_string());
    }
    let (mut stdout, mut stderr): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    let exit = crate::execute_boxed(argv, env, &mut stdout, &mut stderr).await;
    let printed = String::from_utf8_lossy(&stdout).into_owned();
    let envelope = printed.lines().rev().find(|text| !text.trim().is_empty()).and_then(|text| serde_json::from_str::<Value>(text).ok()).unwrap_or(Value::Null);
    (exit, envelope)
}

/// Replaces every recorded id inside `word` by the id the replay created for it.
fn rewrite_ids(word: &str, id_map: &BTreeMap<String, String>) -> String {
    let mut text = word.to_string();
    for (old, new) in id_map {
        if old.len() >= MIN_REWRITTEN_ID_CHARS && old != new && text.contains(old.as_str()) {
            text = text.replace(old.as_str(), new);
        }
    }
    text
}

/// `--strict`: the recorded and replayed data differ once the id pairs are normalised to the same tokens.
fn data_difference(line: &JournalLine, envelope: &Value, id_map: &BTreeMap<String, String>) -> Option<Value> {
    let normalise = |text: String, replayed_side: bool| {
        let mut text = text;
        for (index, (old, new)) in id_map.iter().enumerate() {
            let from = if replayed_side { new } else { old };
            if from.len() >= MIN_REWRITTEN_ID_CHARS {
                text = text.replace(from.as_str(), &format!("<id{index}>"));
            }
        }
        text
    };
    let recorded = line.data_small.as_ref().map(|data| data.to_string()).unwrap_or_default();
    let replayed = envelope.get("data").filter(|data| !data.is_null()).map(|data| journal::data_small(data).to_string()).unwrap_or_default();
    let (recorded, replayed) = (normalise(recorded, false), normalise(replayed, true));
    (recorded != replayed).then(|| json!({"recorded_digest": blake3::hash(recorded.as_bytes()).to_hex().to_string(), "replayed_digest": blake3::hash(replayed.as_bytes()).to_hex().to_string()}))
}
