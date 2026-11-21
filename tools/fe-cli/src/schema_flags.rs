//! JSON-schema -> flag specs and flag parsing, shared by core verbs and `fe x` (S4).

use std::collections::BTreeMap;

use serde_json::{Map, Number, Value};

use crate::envelope::{CliError, ExitCode};
use crate::render::terminal_safe;

/// Flag spellings `parse_flags` handles itself; no schema property may claim them.
const RESERVED_FLAGS: [&str; 5] = ["args", "args-json", "help", "list", "json"];

/// Value type of a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    String,
    Integer,
    Number,
    Boolean,
    /// Nested object or array of objects: `--name-json`.
    Json,
}

/// One generated flag.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagSpec {
    /// Schema property name.
    pub name: String,
    /// Flag spelling without dashes.
    pub flag: String,
    pub kind: FlagKind,
    pub required: bool,
    pub enum_values: Vec<String>,
    pub description: String,
    /// Context field that fills it when absent (`verse`, `fractal`, `petal`).
    pub from_context: Option<String>,
    /// Array of scalars: repeatable.
    pub repeatable: bool,
}

/// Generates flag specs from an `inputSchema` / bus `args_schema`; colliding properties are left to `--args-json`.
pub fn args_from_schema(schema: &Value) -> Vec<FlagSpec> {
    build_specs(schema).0
}

/// Like [`args_from_schema`] but refuses a schema whose properties collide after hyphen mapping (exit 6).
pub fn try_args_from_schema(schema: &Value) -> Result<Vec<FlagSpec>, CliError> {
    let (specs, collision) = build_specs(schema);
    match collision {
        Some(error) => Err(error),
        None => Ok(specs),
    }
}

/// Schema properties no generated flag reaches (unsafe name or collision): `--args-json` only.
pub fn args_json_only(schema: &Value, specs: &[FlagSpec]) -> Vec<String> {
    let mut names: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().filter(|name| !specs.iter().any(|spec| &spec.name == *name)).cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Parses argv into an args object; supports `--args-json` and `--args @file` (explicit flags override them).
pub fn parse_flags(specs: &[FlagSpec], argv: &[String]) -> Result<Map<String, Value>, CliError> {
    let mut lookup: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    for (index, spec) in specs.iter().enumerate() {
        lookup.insert(spec.flag.clone(), (index, false));
        if is_plain_boolean(spec) {
            lookup.insert(format!("no-{}", spec.flag), (index, true));
        }
    }

    let mut base: Option<Map<String, Value>> = None;
    let mut flagged: Map<String, Value> = Map::new();
    let mut index = 0;
    while index < argv.len() {
        let token = &argv[index];
        index += 1;
        let Some(body) = token.strip_prefix("--").filter(|body| !body.is_empty()) else {
            return Err(usage("unexpected-argument", format!("unexpected argument {token}")));
        };
        let (flag_name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (body, None),
        };
        match flag_name {
            "args-json" | "args" => {
                if base.is_some() {
                    return Err(usage("invalid-input", "--args-json/--args given more than once"));
                }
                let raw = take_value(inline, argv, &mut index, flag_name)?;
                let text = if flag_name == "args" {
                    let Some(path) = raw.strip_prefix('@') else {
                        return Err(usage("invalid-input", "--args takes @<file>"));
                    };
                    std::fs::read_to_string(path)
                        .map_err(|error| usage("invalid-input", format!("--args cannot read {path}: {error}")))?
                } else {
                    raw
                };
                base = Some(parse_object(&text, flag_name)?);
            }
            _ => {
                let Some(&(spec_index, negated)) = lookup.get(flag_name) else {
                    return Err(usage("unknown-flag", format!("unknown flag --{flag_name}")));
                };
                let spec = &specs[spec_index];
                if is_plain_boolean(spec) {
                    let value = if negated {
                        if inline.is_some() {
                            return Err(usage("invalid-input", format!("--{flag_name} takes no value")));
                        }
                        false
                    } else {
                        match inline.as_deref() {
                            None | Some("true") => true,
                            Some("false") => false,
                            Some(other) => {
                                return Err(usage("invalid-input", format!("--{flag_name} expects true or false, got {other}")));
                            }
                        }
                    };
                    insert_once(&mut flagged, spec, Value::Bool(value), flag_name)?;
                } else {
                    let raw = take_value(inline, argv, &mut index, flag_name)?;
                    let value = parse_scalar(spec, &raw, flag_name)?;
                    if spec.repeatable {
                        let slot = flagged.entry(spec.name.clone()).or_insert_with(|| Value::Array(Vec::new()));
                        if let Some(items) = slot.as_array_mut() {
                            items.push(value);
                        }
                    } else {
                        insert_once(&mut flagged, spec, value, flag_name)?;
                    }
                }
            }
        }
    }

    let mut args = base.unwrap_or_default();
    for (name, value) in flagged {
        args.insert(name, value);
    }
    for spec in specs {
        if spec.required && spec.from_context.is_none() && !args.contains_key(&spec.name) {
            return Err(usage("missing-required", format!("missing required --{}", spec.flag)));
        }
    }
    Ok(args)
}

/// Human help lines for a flag set, one flag per line (raw text; render through `terminal_safe`).
pub fn flag_help(specs: &[FlagSpec]) -> String {
    let mut lines = Vec::new();
    for spec in specs {
        let head = match spec.kind {
            FlagKind::Boolean if !spec.repeatable => format!("--{0} / --no-{0}", spec.flag),
            FlagKind::Boolean => format!("--{} <true|false>", spec.flag),
            FlagKind::String => format!("--{} <string>", spec.flag),
            FlagKind::Integer => format!("--{} <integer>", spec.flag),
            FlagKind::Number => format!("--{} <number>", spec.flag),
            FlagKind::Json => format!("--{} <json>", spec.flag),
        };
        let mut notes = Vec::new();
        if spec.required {
            notes.push("required".to_string());
        }
        if spec.repeatable {
            notes.push("repeatable".to_string());
        }
        // Schema text may come from an extension: single-lined so it cannot forge a help line.
        if !spec.enum_values.is_empty() {
            notes.push(format!("one of {}", terminal_safe(&spec.enum_values.join("|"))));
        }
        if !spec.description.is_empty() {
            notes.push(terminal_safe(&spec.description));
        }
        lines.push(if notes.is_empty() { format!("  {head}") } else { format!("  {head}  {}", notes.join("; ")) });
    }
    lines.push("  --args-json <json>  whole arguments object (explicit flags override it)".to_string());
    lines.push("  --args @<file>  whole arguments object from a file".to_string());
    lines.join("\n")
}

fn usage(code: &str, message: impl Into<String>) -> CliError {
    CliError::new(ExitCode::Usage, code, message)
}

fn is_plain_boolean(spec: &FlagSpec) -> bool {
    spec.kind == FlagKind::Boolean && !spec.repeatable
}

/// `^[a-z][a-z0-9_-]{0,63}$`.
fn is_flag_safe_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 64
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-')
}

/// Flag specs plus the first collision error; colliding specs are removed from the list.
fn build_specs(schema: &Value) -> (Vec<FlagSpec>, Option<CliError>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return (Vec::new(), None);
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for reserved in RESERVED_FLAGS {
        claims.insert(reserved.to_string(), vec!["(reserved)".to_string()]);
    }
    let mut specs = Vec::new();
    for (name, property) in properties {
        if !is_flag_safe_name(name) {
            continue;
        }
        let spec = spec_for(name, property, required.contains(&name.as_str()));
        claims.entry(spec.flag.clone()).or_default().push(name.clone());
        if is_plain_boolean(&spec) {
            claims.entry(format!("no-{}", spec.flag)).or_default().push(name.clone());
        }
        specs.push(spec);
    }

    let collisions: Vec<(&String, &Vec<String>)> = claims.iter().filter(|(_, owners)| owners.len() > 1).collect();
    let error = collisions.first().map(|(spelling, owners)| {
        usage(
            "flag-collision",
            format!("properties {} all map to --{spelling}; reach them with --args-json", owners.join(", ")),
        )
    });
    specs.retain(|spec| {
        let mut spellings = vec![spec.flag.clone()];
        if is_plain_boolean(spec) {
            spellings.push(format!("no-{}", spec.flag));
        }
        !spellings.iter().any(|spelling| collisions.iter().any(|(taken, _)| *taken == spelling))
    });
    (specs, error)
}

fn spec_for(name: &str, property: &Value, required: bool) -> FlagSpec {
    let spelling = name.replace('_', "-");
    let (kind, repeatable, enum_node) = match type_of(property) {
        "integer" => (FlagKind::Integer, false, property),
        "number" => (FlagKind::Number, false, property),
        "boolean" => (FlagKind::Boolean, false, property),
        "object" => (FlagKind::Json, false, property),
        "array" => match property.get("items") {
            Some(items) => match type_of(items) {
                "integer" => (FlagKind::Integer, true, items),
                "number" => (FlagKind::Number, true, items),
                "boolean" => (FlagKind::Boolean, true, items),
                "string" => (FlagKind::String, true, items),
                _ => (FlagKind::Json, false, property),
            },
            None => (FlagKind::Json, false, property),
        },
        _ => (FlagKind::String, false, property),
    };
    let flag = if kind == FlagKind::Json { format!("{spelling}-json") } else { spelling };
    let from_context = match name {
        "verse_id" => Some("verse".to_string()),
        "fractal_id" => Some("fractal".to_string()),
        "petal_id" => Some("petal".to_string()),
        _ => None,
    };
    FlagSpec {
        name: name.to_string(),
        flag,
        kind,
        required,
        enum_values: if kind == FlagKind::Json { Vec::new() } else { enum_texts(enum_node) },
        description: property.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
        from_context,
        repeatable,
    }
}

/// Schema `type` as a string; arrays take the first non-null entry; unknown means string.
fn type_of(node: &Value) -> &str {
    match node.get("type") {
        Some(Value::String(text)) => text,
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).find(|name| *name != "null").unwrap_or("string"),
        _ => "string",
    }
}

fn enum_texts(node: &Value) -> Vec<String> {
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        return values.iter().map(value_text).collect();
    }
    node.get("const").map(|value| vec![value_text(value)]).unwrap_or_default()
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn take_value(inline: Option<String>, argv: &[String], index: &mut usize, flag: &str) -> Result<String, CliError> {
    if let Some(value) = inline {
        return Ok(value);
    }
    let Some(next) = argv.get(*index) else {
        return Err(usage("invalid-input", format!("--{flag} needs a value")));
    };
    *index += 1;
    Ok(next.clone())
}

fn insert_once(target: &mut Map<String, Value>, spec: &FlagSpec, value: Value, flag: &str) -> Result<(), CliError> {
    if target.contains_key(&spec.name) {
        return Err(usage("invalid-input", format!("--{flag} conflicts with an earlier flag for the same property")));
    }
    target.insert(spec.name.clone(), value);
    Ok(())
}

fn parse_object(text: &str, flag: &str) -> Result<Map<String, Value>, CliError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(usage("invalid-input", format!("--{flag} must be a JSON object"))),
        Err(error) => Err(usage("invalid-input", format!("--{flag} is not valid JSON: {error}"))),
    }
}

fn parse_scalar(spec: &FlagSpec, raw: &str, flag: &str) -> Result<Value, CliError> {
    let bad = |expected: &str| usage("invalid-input", format!("--{flag} expects {expected}, got {raw}"));
    let value = match spec.kind {
        FlagKind::Json => {
            return serde_json::from_str::<Value>(raw)
                .map_err(|error| usage("invalid-input", format!("--{flag} is not valid JSON: {error}")));
        }
        FlagKind::String => Value::String(raw.to_string()),
        FlagKind::Integer => match raw.parse::<i64>() {
            Ok(number) => Value::Number(Number::from(number)),
            Err(_) => match raw.parse::<u64>() {
                Ok(number) => Value::Number(Number::from(number)),
                Err(_) => return Err(bad("an integer")),
            },
        },
        FlagKind::Number => match raw.parse::<f64>() {
            Ok(number) if number.is_finite() => match Number::from_f64(number) {
                Some(number) => Value::Number(number),
                None => return Err(bad("a finite number")),
            },
            _ => return Err(bad("a finite number")),
        },
        FlagKind::Boolean => match raw {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(bad("true or false")),
        },
    };
    if !spec.enum_values.is_empty() && !spec.enum_values.contains(&value_text(&value)) {
        return Err(usage(
            "invalid-input",
            format!("--{flag} must be one of {}, got {raw}", spec.enum_values.join("|")),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn sample_schema() -> Value {
        json!({
            "type": "object",
            "required": ["count"],
            "properties": {
                "count": {"type": "integer"},
                "label": {"type": "string", "description": "free text"},
                "mode": {"type": "string", "enum": ["fast", "slow"]},
                "dry_run": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "origin": {"type": "object"},
                "ratio": {"type": "number"},
                "Bad Name": {"type": "string"}
            }
        })
    }

    #[test]
    fn flags_follow_the_generation_rules() {
        let specs = args_from_schema(&sample_schema());
        let by_flag = |flag: &str| specs.iter().find(|spec| spec.flag == flag);
        assert_eq!(by_flag("count").map(|spec| (spec.kind, spec.required)), Some((FlagKind::Integer, true)));
        assert_eq!(by_flag("dry-run").map(|spec| spec.name.as_str()), Some("dry_run"));
        assert_eq!(by_flag("tags").map(|spec| (spec.kind, spec.repeatable)), Some((FlagKind::String, true)));
        assert_eq!(by_flag("origin-json").map(|spec| spec.kind), Some(FlagKind::Json));
        assert_eq!(by_flag("mode").map(|spec| spec.enum_values.clone()), Some(vec!["fast".to_string(), "slow".to_string()]));
        assert!(specs.iter().all(|spec| spec.name != "Bad Name"), "unsafe names are args-json only");
    }

    #[test]
    fn parse_table() {
        let specs = args_from_schema(&sample_schema());
        let cases: Vec<(Vec<&str>, Result<Value, &str>)> = vec![
            (vec!["--count", "3", "--label", "a b"], Ok(json!({"count": 3, "label": "a b"}))),
            (vec!["--count=3", "--dry-run"], Ok(json!({"count": 3, "dry_run": true}))),
            (vec!["--count", "3", "--no-dry-run"], Ok(json!({"count": 3, "dry_run": false}))),
            (vec!["--count", "3", "--tags", "a", "--tags", "b"], Ok(json!({"count": 3, "tags": ["a", "b"]}))),
            (vec!["--count", "3", "--origin-json", "{\"x\":1}"], Ok(json!({"count": 3, "origin": {"x": 1}}))),
            (vec!["--count", "-4", "--ratio", "2.5"], Ok(json!({"count": -4, "ratio": 2.5}))),
            (vec!["--args-json", "{\"count\":9,\"Bad Name\":\"v\"}"], Ok(json!({"count": 9, "Bad Name": "v"}))),
            (vec!["--args-json", "{\"count\":9}", "--count", "4"], Ok(json!({"count": 4}))),
            (vec!["--label", "x"], Err("--count")),
            (vec!["--count", "3", "--mode", "medium"], Err("--mode")),
            (vec!["--count", "1.5"], Err("--count")),
            (vec!["--count", "3", "--ratio", "NaN"], Err("--ratio")),
            (vec!["--count", "3", "--ratio", "inf"], Err("--ratio")),
            (vec!["--count", "3", "--bogus"], Err("--bogus")),
            (vec!["--count", "3", "--count", "4"], Err("--count")),
            (vec!["--count"], Err("--count")),
        ];
        for (words, expected) in cases {
            let got = parse_flags(&specs, &argv(&words));
            match expected {
                Ok(value) => assert_eq!(Some(&got.expect("parses")), value.as_object(), "{words:?}"),
                Err(flag) => {
                    let error = got.expect_err("refused");
                    assert_eq!(error.exit, ExitCode::Usage, "{words:?}");
                    assert!(error.message.contains(flag), "{words:?}: {}", error.message);
                }
            }
        }
    }

    #[test]
    fn colliding_properties_are_refused() {
        let schema = json!({"properties": {"a_b": {"type": "string"}, "a-b": {"type": "string"}, "keep": {"type": "string"}}});
        let error = try_args_from_schema(&schema).expect_err("collision");
        assert_eq!(error.code, "flag-collision");
        let specs = args_from_schema(&schema);
        let flags: Vec<String> = specs.iter().map(|spec| spec.flag.clone()).collect();
        assert_eq!(flags, vec!["keep".to_string()]);
        assert_eq!(args_json_only(&schema, &specs), vec!["a-b".to_string(), "a_b".to_string()], "dropped properties are listed, never lost");
    }
}
