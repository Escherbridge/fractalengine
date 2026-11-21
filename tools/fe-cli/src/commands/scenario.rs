//! `fe replay --emit-scenario`: journal lines to a `tools/qa/sim` scenario (see AGENTS.md §replay).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::replay::{agent_key, skip_reason};
use crate::catalog;
use crate::envelope::{CliError, ExitCode};
use crate::journal::JournalLine;

/// The simulator's scenario schema allows at most this many peers.
const MAX_SCENARIO_PEERS: usize = 8;
/// `within_s` of every agree expectation.
const AGREE_WITHIN_SECONDS: u64 = 15;
/// `settle_s` of a step that carries expectations (above `within_s`, so the expectation decides the wait).
const CHECKED_STEP_SETTLE_SECONDS: u64 = 20;

/// Rewrites journal ids as scenario references: created ids become `$c<n>.id`, the first verse and two petals
/// the fixture's, and everything else stays verbatim and is reported.
struct Binder {
    created: BTreeMap<String, String>,
    fixture_verse: Option<String>,
    fixture_petals: Vec<String>,
    unbound: BTreeSet<String>,
    /// A redacted placeholder was met since the last reset: the action cannot be emitted.
    redacted: bool,
}

impl Binder {
    fn bind(&mut self, value: &Value, key: Option<&str>) -> Value {
        match value {
            Value::String(text) => {
                if text.contains("<redacted") {
                    self.redacted = true;
                    return value.clone();
                }
                if let Some(reference) = self.created.get(text) {
                    return Value::String(reference.clone());
                }
                match key {
                    Some("verse_id") => self.fixture(text, true),
                    Some("petal_id") => self.fixture(text, false),
                    Some(key) if key.ends_with("_id") && looks_like_id(text) => {
                        self.unbound.insert(text.clone());
                        value.clone()
                    }
                    _ => value.clone(),
                }
            }
            Value::Array(items) => Value::Array(items.iter().map(|item| self.bind(item, key)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(name, inner)| (name.clone(), self.bind(inner, Some(name.as_str())))).collect()),
            other => other.clone(),
        }
    }

    /// Maps the first verse and the first two petals onto the fixture namespace; others are unbound.
    fn fixture(&mut self, id: &str, verse: bool) -> Value {
        if verse {
            let known = self.fixture_verse.get_or_insert_with(|| id.to_string());
            if known.as_str() == id {
                return json!("$fixture.verse_id");
            }
        } else {
            let index = match self.fixture_petals.iter().position(|known| known == id) {
                Some(index) => index,
                None => {
                    self.fixture_petals.push(id.to_string());
                    self.fixture_petals.len() - 1
                }
            };
            if let Some(reference) = ["$fixture.petal_a_id", "$fixture.petal_b_id"].get(index) {
                return json!(reference);
            }
        }
        self.unbound.insert(id.to_string());
        json!(id)
    }
}

fn looks_like_id(text: &str) -> bool {
    text.len() >= 8 && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Scenario peer names are `^[a-z][a-z0-9]{0,15}$`.
fn peer_name(raw: &str) -> String {
    let mut name: String = raw.chars().filter(char::is_ascii_alphanumeric).map(|character| character.to_ascii_lowercase()).collect();
    if !name.starts_with(|character: char| character.is_ascii_lowercase()) {
        name.insert(0, 'p');
    }
    name.truncate(16);
    name
}

/// Scenario names are `^[a-z][a-z0-9-]{0,47}$`.
pub fn kebab_name(raw: &str) -> String {
    let mut name = String::new();
    for character in raw.chars() {
        let lowered = character.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            name.push(lowered);
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
    }
    let mut name = name.trim_matches('-').to_string();
    if !name.starts_with(|character: char| character.is_ascii_lowercase()) {
        name.insert_str(0, "journal-");
    }
    name.truncate(48);
    name.trim_end_matches('-').to_string()
}

fn role_rank(role: &str) -> u8 {
    match role {
        "owner" => 3,
        "manager" | "editor" => 2,
        _ => 1,
    }
}

/// The sim's expectation for one accepted write: peers must hold the same value (None when nothing observable).
fn agree_expectation(tool: &str, args: &Value, action_id: &str) -> Option<Value> {
    let created = || json!(format!("${action_id}.id"));
    let argument = |key: &str| args.get(key).cloned();
    let (probe, target, path) = match tool {
        "create_node" | "place_asset" => ("node", Some(created()), None),
        "delete_node" | "commit_node_mutation" => ("node", argument("node_id"), None),
        "update_transform" => ("transform", argument("node_id"), Some("position")),
        "set_property" | "delete_property" => ("properties", argument("node_id"), None),
        "set_visibility_group" => ("groups", argument("petal_id"), None),
        "create_petal" => ("hierarchy", Some(created()), None),
        "create_fractal" => ("hierarchy", argument("verse_id"), None),
        _ => return None,
    };
    let target = target?;
    let mut expectation = json!({"probe": probe, "target": target, "predicate": "agree", "within_s": AGREE_WITHIN_SECONDS});
    if let Some(path) = path {
        expectation["path"] = json!(path);
    }
    Some(expectation)
}

/// `(via, op, args)` of a journal line, or why it has no scenario action.
fn action_of(line: &JournalLine) -> Result<(&'static str, String, Value), &'static str> {
    let args = line.resolved.args.clone().filter(Value::is_object).unwrap_or_else(|| json!({}));
    match (line.resolved.via.as_str(), line.resolved.tool.as_deref()) {
        ("mcp", Some("get_hierarchy")) => Err("hierarchy reads are skipped (each one rebuilds the GUI tree)"),
        ("mcp", Some(tool)) => Ok(("mcp", tool.to_string(), args)),
        ("rest", None) if line.resolved.verb == "x" => Err("extension commands need installed packages"),
        ("rest", None) => {
            let (Some(method), Some(template)) = (line.resolved.method.clone(), line.resolved.path.clone()) else {
                return Err("REST verb without a recorded route");
            };
            let path = match args.get("petal_id").and_then(Value::as_str) {
                Some(petal_id) => template.replace("{petal_id}", petal_id),
                None => template,
            };
            if path.contains('{') {
                return Err("REST route has an id the journal did not record");
            }
            Ok(("rest", "request".to_string(), json!({"method": method, "path": format!("/api/v1{path}")})))
        }
        _ => Err("no scenario op for this verb"),
    }
}

fn expected_outcome(exit: i32) -> &'static str {
    match exit {
        0 => "ok",
        2 | 4 | 5 | 6 => "refused",
        _ => "any",
    }
}

/// Builds the scenario and a report (`steps`, `actions`, `peers`, `skipped`, `unbound_ids`); exit 6 when the journal cannot form one.
pub fn emit(lines: &[JournalLine], scenario_name: &str, source: &str) -> Result<(Value, Value), CliError> {
    let usage = |code: &str, message: String| CliError::new(ExitCode::Usage, code, message);

    // Peers: one per journal agent, the first owner named `owner` (the sim requires it first).
    let mut agents: Vec<(String, u8)> = Vec::new();
    for line in lines.iter().filter(|line| skip_reason(line).is_none() && action_of(line).is_ok()) {
        let key = agent_key(line);
        let rank = role_rank(line.instance.role.as_deref().unwrap_or("viewer").to_ascii_lowercase().as_str());
        match agents.iter_mut().find(|(known, _)| *known == key) {
            Some((_, best)) => *best = (*best).max(rank),
            None => agents.push((key, rank)),
        }
    }
    let Some(owner_agent) = agents.iter().find(|(_, rank)| *rank == 3).map(|(agent, _)| agent.clone()) else {
        return Err(usage("scenario-needs-owner", "no agent in the journal acted as an owner; a scenario needs one".to_string()));
    };
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut peers: Vec<Value> = vec![json!({"name": "owner", "role": "owner"})];
    names.insert(owner_agent.clone(), "owner".to_string());
    for (agent, rank) in agents.iter().filter(|(agent, _)| *agent != owner_agent) {
        let mut name = peer_name(agent);
        let mut suffix = 2;
        while name == "owner" || names.values().any(|taken| *taken == name) {
            name = format!("{}{suffix}", peer_name(agent).chars().take(14).collect::<String>());
            suffix += 1;
        }
        let role = if *rank >= 2 { "editor" } else { "viewer" };
        peers.push(json!({"name": name, "role": role}));
        names.insert(agent.clone(), name);
    }
    if peers.len() < 2 {
        peers.push(json!({"name": "observer", "role": "viewer"}));
    }
    if peers.len() > MAX_SCENARIO_PEERS {
        return Err(usage("scenario-too-many-peers", format!("the journal has {} agents; a scenario takes at most {MAX_SCENARIO_PEERS}", peers.len())));
    }

    let mut binder = Binder { created: BTreeMap::new(), fixture_verse: None, fixture_petals: Vec::new(), unbound: BTreeSet::new(), redacted: false };
    let (mut steps, mut actions, mut expectations, mut skipped) = (Vec::<Value>::new(), Vec::<Value>::new(), Vec::<Value>::new(), Vec::<Value>::new());
    let mut action_count = 0usize;
    let mut step_title = String::new();
    for line in lines {
        let skip = |reason: &str| json!({"seq": line.seq, "verb": line.resolved.verb, "reason": reason});
        if let Some(reason) = skip_reason(line) {
            skipped.push(skip(reason));
            continue;
        }
        let (via, op, args) = match action_of(line) {
            Ok(parts) => parts,
            Err(reason) => {
                skipped.push(skip(reason));
                continue;
            }
        };
        binder.redacted = false;
        let bound = binder.bind(&args, None);
        if binder.redacted {
            skipped.push(skip("an argument is redacted in the journal"));
            continue;
        }
        action_count += 1;
        let action_id = format!("c{action_count}");
        let peer = names.get(&agent_key(line)).cloned().unwrap_or_else(|| "owner".to_string());
        if step_title.is_empty() {
            step_title = format!("{} ({peer})", line.resolved.verb);
        }
        actions.push(json!({"id": action_id, "peer": peer, "via": via, "op": op, "args": bound, "expect": expected_outcome(line.exit)}));
        if line.exit != 0 {
            continue;
        }
        if let Some(created) = line.created_ids.first() {
            binder.created.insert(created.clone(), format!("${action_id}.id"));
        }
        let is_write = line
            .resolved
            .verb
            .split_once(' ')
            .or(Some((line.resolved.verb.as_str(), "")))
            .and_then(|(noun, verb)| catalog::find_verb(noun, verb))
            .is_some_and(catalog::is_write);
        if is_write {
            expectations.extend(agree_expectation(&op, &bound, &action_id));
            let mut step = json!({"id": format!("s{}", steps.len() + 1), "title": step_title, "settle_s": if expectations.is_empty() { 1 } else { CHECKED_STEP_SETTLE_SECONDS }, "actions": actions});
            if !expectations.is_empty() {
                step["expect"] = Value::Array(std::mem::take(&mut expectations));
            }
            steps.push(step);
            actions = Vec::new();
            step_title = String::new();
        }
    }
    if !actions.is_empty() {
        steps.push(json!({"id": format!("s{}", steps.len() + 1), "title": step_title, "settle_s": 1, "actions": actions}));
    }
    if steps.is_empty() {
        return Err(usage("scenario-empty", "the journal holds no step a scenario can run".to_string()));
    }

    let peer_names: Vec<Value> = peers.iter().filter_map(|peer| peer.get("name").cloned()).collect();
    let mut scenario = Map::new();
    scenario.insert("name".to_string(), json!(scenario_name));
    scenario.insert("description".to_string(), json!(format!("Emitted by `fe replay --emit-scenario` from {source}; each accepted node write carries an agree expectation.")));
    scenario.insert("seed".to_string(), json!(1));
    scenario.insert("fixture".to_string(), json!("basic"));
    scenario.insert("peers".to_string(), Value::Array(peers));
    scenario.insert("steps".to_string(), Value::Array(steps.clone()));
    let report = json!({
        "steps": steps.len(),
        "actions": action_count,
        "peers": peer_names,
        "skipped": skipped,
        "unbound_ids": binder.unbound.iter().collect::<Vec<_>>(),
    });
    Ok((Value::Object(scenario), report))
}
