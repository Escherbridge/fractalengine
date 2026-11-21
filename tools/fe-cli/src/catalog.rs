//! Table-driven verb map and the embedded MCP tool snapshot (S3).
//! Snapshot row format: `min_role` and `scope_rule` are the `Debug` text of `RoleLevel` / `ScopeRule`.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope::{CliError, ExitCode};
use crate::schema_flags::{args_from_schema, FlagKind, FlagSpec};

/// One row of `catalog/mcp-tools.json` (mirror of `fe_api::mcp::tool_specs()`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolRow {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub min_role: Value,
    pub scope_rule: Value,
}

/// Which door a verb uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// MCP tool name.
    Mcp(&'static str),
    /// REST method and path template.
    Rest(&'static str, &'static str),
    /// Handled locally, no network.
    Local,
}

/// One `noun verb` row.
#[derive(Debug, Clone, Copy)]
pub struct Verb {
    pub noun: &'static str,
    pub verb: &'static str,
    pub door: Door,
    pub summary: &'static str,
    /// Implementation phase (1 = ships now).
    pub phase: u8,
}

/// Positional-argument names and context-filled arguments of one verb.
#[derive(Debug, Clone, Copy)]
pub struct VerbDetail {
    /// Schema argument names, in the order bare words fill them.
    pub positionals: &'static [&'static str],
    /// `(argument name, context field)`: filled from the context unless a flag is given.
    pub from_context: &'static [(&'static str, &'static str)],
}

const SNAPSHOT: &str = include_str!("../catalog/mcp-tools.json");

static VERBS: &[Verb] = &[
    Verb { noun: "instance", verb: "list", door: Door::Local, summary: "List discoverable local instances", phase: 1 },
    Verb { noun: "use", verb: "instance", door: Door::Local, summary: "Select the instance for later commands", phase: 1 },
    Verb { noun: "use", verb: "verse", door: Door::Mcp("get_hierarchy"), summary: "Select a verse by id or name (CLI context only)", phase: 1 },
    Verb { noun: "use", verb: "fractal", door: Door::Mcp("get_hierarchy"), summary: "Select a fractal by id or name (CLI context only)", phase: 1 },
    Verb { noun: "use", verb: "petal", door: Door::Mcp("get_hierarchy"), summary: "Select a petal by id or name (CLI context only)", phase: 1 },
    Verb { noun: "ctx", verb: "", door: Door::Local, summary: "Show the current context", phase: 1 },
    Verb { noun: "ctx", verb: "clear", door: Door::Local, summary: "Clear verse, fractal and petal from the context", phase: 1 },
    Verb { noun: "verse", verb: "list", door: Door::Mcp("get_hierarchy"), summary: "List verses (union over every verse token)", phase: 1 },
    Verb { noun: "verse", verb: "create", door: Door::Mcp("create_verse"), summary: "Create a verse", phase: 1 },
    Verb { noun: "fractal", verb: "list", door: Door::Mcp("get_hierarchy"), summary: "List fractals of the current verse", phase: 1 },
    Verb { noun: "fractal", verb: "create", door: Door::Mcp("create_fractal"), summary: "Create a fractal in the current verse", phase: 1 },
    Verb { noun: "petal", verb: "list", door: Door::Mcp("get_hierarchy"), summary: "List petals of the current fractal", phase: 1 },
    Verb { noun: "petal", verb: "create", door: Door::Mcp("create_petal"), summary: "Create a petal in the current fractal", phase: 1 },
    Verb { noun: "node", verb: "list", door: Door::Rest("GET", "/petals/{petal_id}/nodes"), summary: "List nodes of the current petal (--kind pages the typed REST list)", phase: 1 },
    Verb { noun: "node", verb: "create", door: Door::Mcp("create_node"), summary: "Create a node; --asset-id places an uploaded asset instead", phase: 1 },
    Verb { noun: "node", verb: "get", door: Door::Mcp("read_node"), summary: "Read one node", phase: 1 },
    Verb { noun: "node", verb: "move", door: Door::Mcp("update_transform"), summary: "Set a node's position, rotation or scale", phase: 1 },
    Verb { noun: "node", verb: "set", door: Door::Mcp("set_property"), summary: "Set one custom property on a node", phase: 1 },
    Verb { noun: "node", verb: "unset", door: Door::Mcp("delete_property"), summary: "Delete one custom property from a node", phase: 1 },
    Verb { noun: "node", verb: "delete", door: Door::Mcp("delete_node"), summary: "Tombstone a node (--cascade for its subtree)", phase: 1 },
    Verb { noun: "help", verb: "", door: Door::Local, summary: "Show help (--json for the machine catalog)", phase: 1 },
    Verb { noun: "note", verb: "", door: Door::Local, summary: "Record a friction note in the journal", phase: 1 },
    // Phase 2: one row per MCP tool or REST route; `commands::generic` runs every one of them.
    Verb { noun: "node", verb: "props", door: Door::Mcp("get_properties"), summary: "List a node's custom properties", phase: 2 },
    Verb { noun: "node", verb: "history", door: Door::Mcp("get_node_history"), summary: "Page a node's commit history", phase: 2 },
    Verb { noun: "asset", verb: "upload", door: Door::Mcp("upload_asset"), summary: "Upload a GLB file (max 11 MiB) and print its asset id", phase: 2 },
    Verb { noun: "terrain", verb: "get", door: Door::Rest("GET", "/petals/{petal_id}/terrain"), summary: "Show the current petal's terrain config", phase: 2 },
    Verb { noun: "terrain", verb: "set", door: Door::Mcp("set_petal_terrain"), summary: "Set the petal's terrain config (--terrain-json or --args @file)", phase: 2 },
    Verb { noun: "terrain", verb: "clear", door: Door::Mcp("set_petal_terrain"), summary: "Clear the petal's terrain config", phase: 2 },
    Verb { noun: "group", verb: "list", door: Door::Mcp("list_visibility_groups"), summary: "List the petal's visibility groups", phase: 2 },
    Verb { noun: "group", verb: "set", door: Door::Mcp("set_visibility_group"), summary: "Create or update a visibility group", phase: 2 },
    Verb { noun: "group", verb: "delete", door: Door::Mcp("set_visibility_group"), summary: "Delete a visibility group", phase: 2 },
    Verb { noun: "invite", verb: "create", door: Door::Mcp("issue_verse_invite"), summary: "Invite a peer to the current verse (--to-instance <name> or --recipient-did)", phase: 2 },
    Verb { noun: "join", verb: "", door: Door::Mcp("join_verse"), summary: "Join a verse from an invite (--invite - reads stdin)", phase: 2 },
    Verb { noun: "role", verb: "list", door: Door::Mcp("list_roles"), summary: "List roles and members of the current verse", phase: 2 },
    Verb { noun: "role", verb: "assign", door: Door::Mcp("assign_role"), summary: "Assign a role to a peer DID", phase: 2 },
    Verb { noun: "role", verb: "revoke", door: Door::Mcp("revoke_role"), summary: "Revoke a peer DID's role", phase: 2 },
    Verb { noun: "net", verb: "diag", door: Door::Mcp("get_network_diagnostics"), summary: "Show peer connectivity (addresses redacted)", phase: 2 },
    Verb { noun: "identity", verb: "", door: Door::Mcp("get_identity"), summary: "Show this node's DID and relay state", phase: 2 },
    Verb { noun: "sync", verb: "state", door: Door::Mcp("get_sync_state"), summary: "Show replica state of the current verse (owner, diagnostics gate)", phase: 2 },
    Verb { noun: "query", verb: "", door: Door::Mcp("query"), summary: "Run one read-only SurrealQL SELECT", phase: 2 },
    Verb { noun: "replay", verb: "", door: Door::Local, summary: "Re-run a journal with no LLM; --emit-scenario writes a sim scenario instead", phase: 2 },
];

/// Highest phase whose rows are listed and routed.
pub const SHIPPED_PHASE: u8 = 2;

/// Global flag spellings the spine consumes anywhere before the noun's tail; no schema property may claim them.
pub const GLOBAL_FLAGS: &[&str] =
    &["json", "dry-run", "help", "version", "instance", "wait", "wait-budget", "retry", "verse", "fractal", "petal"];

/// Nouns handled outside the verb table.
const EXTRA_NOUNS: &[&str] = &["x"];

/// The full verb table.
pub fn verb_table() -> &'static [Verb] {
    VERBS
}

/// Finds a verb by noun and verb name (`""` for noun-only commands such as `help`).
pub fn find_verb(noun: &str, verb: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|row| row.noun == noun && row.verb == verb)
}

/// `["noun"]` or `["noun", "verb"]`.
pub fn path_of(verb: &Verb) -> Vec<&'static str> {
    if verb.verb.is_empty() { vec![verb.noun] } else { vec![verb.noun, verb.verb] }
}

/// Whether the verb changes state (accepts `--wait`).
pub fn is_write(verb: &Verb) -> bool {
    verb.door != Door::Local
        && (matches!(verb.verb, "create" | "move" | "set" | "unset" | "delete" | "clear" | "assign" | "revoke" | "upload") || verb.noun == "join")
}

/// Resolves the command words (not `x`) to one table row and the number of words it consumed; anything else exits 6
/// naming the nearest valid commands.
pub fn route(words: &[String]) -> Result<(&'static Verb, usize), CliError> {
    let noun = words.first().map(String::as_str).unwrap_or_default();
    let next = words.get(1).map(String::as_str).filter(|word| !word.starts_with('-'));
    if let Some(row) = next.and_then(|word| find_verb(noun, word)) {
        return Ok((row, 2));
    }
    if let Some(row) = find_verb(noun, "") {
        let takes_words = !detail(row).positionals.is_empty();
        if next.is_none() || takes_words {
            return Ok((row, 1));
        }
    }
    let attempted = match next {
        Some(word) => format!("fe {noun} {word}"),
        None => format!("fe {noun}"),
    };
    let same_noun: Vec<&Verb> = VERBS.iter().filter(|row| row.noun == noun).collect();
    let suggestions: Vec<String> = if same_noun.is_empty() {
        let mut nouns: Vec<&str> = VERBS.iter().map(|row| row.noun).chain(EXTRA_NOUNS.iter().copied()).collect();
        nouns.dedup();
        let near: Vec<&str> = nouns.iter().copied().filter(|candidate| edit_distance(candidate, noun) <= 2).collect();
        let chosen = if near.is_empty() { nouns } else { near };
        chosen.into_iter().map(|candidate| format!("fe {candidate}")).collect()
    } else {
        let mut rows = same_noun;
        if let Some(word) = next {
            rows.sort_by_key(|row| edit_distance(row.verb, word));
        }
        rows.iter().map(|row| format!("fe {}", path_of(row).join(" "))).collect()
    };
    Err(CliError::new(
        ExitCode::Usage,
        "unknown-command",
        format!("unknown command `{attempted}`; nearest: {}", suggestions.join(", ")),
    ))
}

/// Levenshtein edit distance (suggestions only).
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != *right_char);
            current.push(substitution.min(previous[column + 1] + 1).min(current[column] + 1));
        }
        previous = current;
    }
    previous[right.len()]
}

/// Global-scope Owner tools take the operator token, not a verse token (derived from the snapshot `scope_rule`).
pub fn wants_operator_token(tool: &str) -> bool {
    tool_row(tool).is_some_and(|row| row.scope_rule.as_str() == Some("Global") && row.min_role.as_str() == Some("Owner"))
}

fn rows() -> &'static [ToolRow] {
    static ROWS: OnceLock<Vec<ToolRow>> = OnceLock::new();
    ROWS.get_or_init(|| serde_json::from_str(SNAPSHOT).expect("embedded catalog/mcp-tools.json is valid"))
}

/// Parses the embedded snapshot.
pub fn tool_rows() -> Vec<ToolRow> {
    rows().to_vec()
}

/// One snapshot row by tool name.
pub fn tool_row(name: &str) -> Option<&'static ToolRow> {
    rows().iter().find(|row| row.name == name)
}

/// Blake3 digest of the embedded snapshot.
pub fn catalog_digest() -> String {
    blake3::hash(SNAPSHOT.as_bytes()).to_hex().to_string()
}

/// Positionals and context-filled arguments of a verb.
pub fn detail(verb: &Verb) -> VerbDetail {
    let plain = |positionals: &'static [&'static str]| VerbDetail { positionals, from_context: &[] };
    match (verb.noun, verb.verb) {
        ("use", _) => plain(&["target"]),
        ("verse", "create") => plain(&["name"]),
        ("fractal", "create") => VerbDetail { positionals: &["name"], from_context: &[("verse_id", "verse")] },
        ("petal", "create") => VerbDetail { positionals: &["name"], from_context: &[("fractal_id", "fractal")] },
        ("node", "create") => VerbDetail { positionals: &["name"], from_context: &[("petal_id", "petal")] },
        ("node", "list") => VerbDetail { positionals: &[], from_context: &[("petal_id", "petal")] },
        ("node", "get") | ("node", "move") | ("node", "delete") | ("node", "props") | ("node", "history") => plain(&["node_id"]),
        ("asset", "upload") | ("terrain", _) | ("group", "list") => VerbDetail { positionals: &[], from_context: &[("petal_id", "petal")] },
        ("group", _) => VerbDetail { positionals: &["group_id"], from_context: &[("petal_id", "petal")] },
        ("invite", _) | ("role", _) | ("sync", _) => VerbDetail { positionals: &[], from_context: &[("verse_id", "verse")] },
        ("query", _) => plain(&["sql"]),
        ("replay", _) => plain(&["journal"]),
        ("node", "set") => plain(&["node_id", "key", "value"]),
        ("node", "unset") => plain(&["node_id", "key"]),
        ("note", _) => plain(&["text"]),
        _ => plain(&[]),
    }
}

/// Lowercase minimum role of a verb from the snapshot (`None` for local verbs).
pub fn min_role(verb: &Verb) -> Option<String> {
    match verb.door {
        Door::Mcp(tool) => tool_row(tool).and_then(|row| row.min_role.as_str()).map(str::to_ascii_lowercase),
        Door::Rest(..) => Some("viewer".to_string()),
        Door::Local => None,
    }
}

/// Short resource scope label derived from a snapshot `scope_rule`.
pub fn scope_label(rule: &str) -> &'static str {
    match rule {
        "Global" => "global",
        "None" => "token",
        "BindingTarget" => "binding",
        _ if rule.starts_with("PetalArg") || rule == "HierarchyArgs(Petal)" => "petal",
        _ if rule.starts_with("NodeArg") || rule.starts_with("HistoryNodeArg") => "node",
        _ if rule.starts_with("VerseArg") || rule == "HierarchyArgs(Verse)" => "verse",
        _ if rule == "HierarchyArgs(Fractal)" => "fractal",
        _ => "other",
    }
}

/// Scope label of a verb (`None` for local verbs).
pub fn verb_scope(verb: &Verb) -> Option<&'static str> {
    match verb.door {
        Door::Mcp(tool) => tool_row(tool).and_then(|row| row.scope_rule.as_str()).map(scope_label),
        Door::Rest(..) => Some("petal"),
        Door::Local => None,
    }
}

fn plain_spec(name: &str, kind: FlagKind, description: &str) -> FlagSpec {
    FlagSpec {
        name: name.to_string(),
        flag: name.replace('_', "-"),
        kind,
        required: false,
        enum_values: Vec::new(),
        description: description.to_string(),
        from_context: None,
        repeatable: false,
    }
}

fn rest_specs(verb: &Verb) -> Vec<FlagSpec> {
    match (verb.noun, verb.verb) {
        ("node", "list") => {
            let mut kind = plain_spec("kind", FlagKind::String, "Typed node list; omit to list every node of the petal from the hierarchy");
            kind.enum_values = ["stamp", "earthwork_region", "repository"].iter().map(|tag| tag.to_string()).collect();
            vec![
                plain_spec("petal_id", FlagKind::String, "Petal to list"),
                kind,
                plain_spec("path_id", FlagKind::String, "Only stamps owned by this path (needs --kind stamp)"),
                plain_spec("cursor", FlagKind::String, "Opaque `next` value of the previous page"),
                plain_spec("limit", FlagKind::Integer, "Most rows on this page"),
            ]
        }
        ("terrain", "get") => vec![plain_spec("petal_id", FlagKind::String, "Petal to read")],
        _ => Vec::new(),
    }
}

/// Flags of the local verbs that take any (`replay`).
fn local_specs(verb: &Verb) -> Vec<FlagSpec> {
    let repeatable = |name: &str, description: &str| {
        let mut spec = plain_spec(name, FlagKind::String, description);
        spec.repeatable = true;
        spec
    };
    match (verb.noun, verb.verb) {
        ("replay", "") => vec![
            repeatable("map", "agent=instance: run that agent's steps on that instance (default: the current instance)"),
            repeatable("bind", "old=new: seed the id map, e.g. a fixture verse of the instance being replayed against"),
            plain_spec("strict", FlagKind::Boolean, "Also compare each step's data (ids normalised) with the journal"),
            plain_spec("emit_scenario", FlagKind::String, "Write a tools/qa/sim scenario to this path instead of replaying"),
            plain_spec("scenario_name", FlagKind::String, "Scenario name for --emit-scenario (lowercase kebab-case)"),
        ],
        _ => Vec::new(),
    }
}

/// Flag specs of a verb: MCP verbs derive them from the snapshot `inputSchema`.
pub fn flag_specs(verb: &Verb) -> Vec<FlagSpec> {
    let detail = detail(verb);
    let mut specs = match verb.door {
        Door::Mcp("get_hierarchy") => Vec::new(),
        Door::Mcp(tool) => tool_row(tool).map(|row| args_from_schema(&row.input_schema)).unwrap_or_default(),
        Door::Rest(..) => rest_specs(verb),
        Door::Local => local_specs(verb),
    };
    match (verb.noun, verb.verb) {
        ("asset", "upload") => {
            specs.retain(|spec| spec.name != "data_base64");
            for spec in specs.iter_mut().filter(|spec| spec.name == "name") {
                spec.required = false;
                spec.description = "Display name (default: the file name)".to_string();
            }
            specs.push(plain_spec("file", FlagKind::String, "GLB file to upload (max 11 MiB)"));
        }
        ("invite", "create") => {
            for spec in specs.iter_mut().filter(|spec| spec.name == "recipient_did") {
                spec.required = false;
            }
            specs.push(plain_spec("to_instance", FlagKind::String, "Local instance whose DID is the recipient (read with get_identity)"));
        }
        _ => {}
    }
    if (verb.noun, verb.verb) == ("node", "create") {
        if let Some(place) = tool_row("place_asset") {
            for mut extra in args_from_schema(&place.input_schema) {
                if !specs.iter().any(|spec| spec.name == extra.name) {
                    extra.required = false;
                    specs.push(extra);
                }
            }
        }
    }
    if (verb.noun, verb.verb) == ("node", "move") {
        for axis in ["x", "y", "z"] {
            specs.push(plain_spec(axis, FlagKind::Number, "Position component; the rest come from the node"));
        }
        for spec in specs.iter_mut().filter(|spec| ["position", "rotation", "scale"].contains(&spec.name.as_str())) {
            spec.required = false;
        }
    }
    // The global spellings belong to the spine: a property claiming one is reachable only via --args-json.
    specs.retain(|spec| !GLOBAL_FLAGS.contains(&spec.flag.as_str()));
    for spec in specs.iter_mut() {
        if let Some((_, field)) = detail.from_context.iter().find(|(argument, _)| *argument == spec.name) {
            spec.from_context = Some((*field).to_string());
            spec.required = false;
        }
        if detail.positionals.contains(&spec.name.as_str()) {
            spec.required = false;
        }
        // Vectors take one `x,y,z` value (normalised in `commands::merge_args`).
        if ["position", "rotation", "scale"].contains(&spec.name.as_str()) {
            spec.kind = FlagKind::String;
            spec.repeatable = false;
            spec.flag = spec.name.clone();
            if spec.description.is_empty() {
                spec.description = "x,y,z".to_string();
            } else {
                spec.description = format!("x,y,z: {}", spec.description);
            }
        }
    }
    specs
}

/// Schema properties of a verb that no generated flag reaches (name pattern, collision, or a global spelling):
/// they are set only through `--args-json` / `--args @file`.
pub fn args_json_only(verb: &Verb) -> Vec<String> {
    let Door::Mcp(tool) = verb.door else { return Vec::new() };
    let Some(row) = tool_row(tool) else { return Vec::new() };
    crate::schema_flags::args_json_only(&row.input_schema, &flag_specs(verb))
}

fn type_matches(declared: &Value, value: &Value) -> bool {
    let check = |name: &str| match name {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some() || value.as_f64().is_some_and(|number| number.fract() == 0.0),
        _ => true,
    };
    match declared {
        Value::String(name) => check(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).any(check),
        _ => true,
    }
}

/// Validates an argument object against a tool `inputSchema` subset (required, type, enum, array size).
pub fn validate_args(schema: &Value, args: &Value) -> Result<(), String> {
    let Some(map) = args.as_object() else {
        return Err("arguments must be an object".to_string());
    };
    for required in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        let missing = match map.get(required) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(_) => false,
        };
        if missing {
            return Err(format!("{required} is required"));
        }
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };
    for (name, value) in map {
        let Some(property) = properties.get(name) else { continue };
        if let Some(declared) = property.get("type") {
            if !type_matches(declared, value) {
                return Err(format!("invalid {name}: expected {declared}"));
            }
        }
        if let Some(allowed) = property.get("enum").and_then(Value::as_array) {
            if !allowed.contains(value) {
                return Err(format!("invalid {name}: expected one of {}", Value::Array(allowed.clone())));
            }
        }
        if let Some(items) = value.as_array() {
            let length = items.len() as u64;
            if property.get("minItems").and_then(Value::as_u64).is_some_and(|minimum| length < minimum)
                || property.get("maxItems").and_then(Value::as_u64).is_some_and(|maximum| length > maximum)
            {
                return Err(format!("invalid {name}: wrong number of items"));
            }
            if let Some(item_type) = property.get("items").and_then(|items| items.get("type")) {
                if !items.iter().all(|item| type_matches(item_type, item)) {
                    return Err(format!("invalid {name}: expected items of type {item_type}"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_table_integrity() {
        let table = verb_table();
        for (index, verb) in table.iter().enumerate() {
            assert!(
                !table[..index].iter().any(|other| other.noun == verb.noun && other.verb == verb.verb),
                "duplicate path {} {}",
                verb.noun,
                verb.verb
            );
            if let Door::Mcp(tool) = verb.door {
                let row = tool_row(tool).unwrap_or_else(|| panic!("{} {} maps to unknown tool {tool}", verb.noun, verb.verb));
                let properties = row.input_schema.get("properties").and_then(Value::as_object).expect("schema properties");
                for (argument, _) in detail(verb).from_context {
                    assert!(properties.contains_key(*argument), "{tool} has no argument {argument}");
                }
                for positional in detail(verb).positionals.iter().filter(|name| **name != "target") {
                    assert!(properties.contains_key(*positional), "{tool} has no argument {positional}");
                }
            }
        }
        let rows = tool_rows();
        let mut names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), rows.len(), "duplicate tool names in snapshot");

        // Routing is total over the table, and a refusal names the nearest valid commands.
        let words = |text: &str| text.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        for row in table {
            let path = path_of(row).join(" ");
            let (found, consumed) = route(&words(&path)).unwrap_or_else(|error| panic!("{path}: {}", error.message));
            assert_eq!((found.noun, found.verb, consumed), (row.noun, row.verb, path_of(row).len()), "{path}");
        }
        let refusals = [("node mvoe", "fe node move"), ("ndoe list", "fe node"), ("ctx bogus", "fe ctx clear"), ("use", "fe use petal")];
        for (typed, nearest) in refusals {
            let error = route(&words(typed)).expect_err(typed);
            assert_eq!((error.exit, error.code.as_str()), (ExitCode::Usage, "unknown-command"), "{typed}");
            assert!(error.message.contains(nearest), "{typed}: {}", error.message);
        }
        let (note, consumed) = route(&words("note the move verb was hard to find")).expect("note takes words");
        assert_eq!((note.noun, consumed), ("note", 1));
    }
}
