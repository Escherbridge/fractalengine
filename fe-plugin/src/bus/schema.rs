//! Command `args` checked against the `args_schema` subset of bus.md §4 (no `$ref`, combinators, `pattern`).

use serde_json::{Map, Value};

/// Recursion guard; manifests are validated to depth 4, this only bounds hostile input.
const MAX_DEPTH: usize = 8;

/// Check `args` against an optional schema; an absent schema accepts exactly `{}`.
pub fn validate_args(schema: Option<&Value>, args: &Value) -> Result<(), String> {
    match schema {
        None => match args {
            Value::Object(map) if map.is_empty() => Ok(()),
            _ => Err("this command takes no arguments: send {}".into()),
        },
        Some(schema) => check(schema, args, "args", 0),
    }
}

fn check(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: nested too deep"));
    }
    let Some(rules) = schema.as_object() else {
        return Err(format!("{path}: schema is not an object"));
    };
    if let Some(expected) = rules.get("type") {
        check_type(expected, value, path)?;
    }
    if let Some(Value::Array(options)) = rules.get("enum") {
        if !options.iter().any(|option| json_equal(option, value)) {
            return Err(format!("{path}: not one of the allowed values"));
        }
    }
    if let Some(constant) = rules.get("const") {
        if !json_equal(constant, value) {
            return Err(format!("{path}: must equal {constant}"));
        }
    }
    match value {
        Value::Number(number) => check_number(rules, number.as_f64().unwrap_or(f64::NAN), path),
        Value::String(text) => check_length(rules, text.chars().count(), "minLength", "maxLength", path),
        Value::Array(items) => {
            check_length(rules, items.len(), "minItems", "maxItems", path)?;
            if let Some(item_schema) = rules.get("items") {
                for (index, item) in items.iter().enumerate() {
                    check(item_schema, item, &format!("{path}[{index}]"), depth + 1)?;
                }
            }
            Ok(())
        }
        Value::Object(map) => check_object(rules, map, path, depth),
        Value::Bool(_) | Value::Null => Ok(()),
    }
}

fn check_type(expected: &Value, value: &Value, path: &str) -> Result<(), String> {
    let matches = |name: &str| match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|x| x.fract() == 0.0),
        _ => false,
    };
    let ok = match expected {
        Value::String(name) => matches(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).any(matches),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: expected type {expected}"))
    }
}

fn check_number(rules: &Map<String, Value>, number: f64, path: &str) -> Result<(), String> {
    if let Some(minimum) = rules.get("minimum").and_then(Value::as_f64) {
        if number.is_nan() || number < minimum {
            return Err(format!("{path}: below minimum {minimum}"));
        }
    }
    if let Some(maximum) = rules.get("maximum").and_then(Value::as_f64) {
        if number.is_nan() || number > maximum {
            return Err(format!("{path}: above maximum {maximum}"));
        }
    }
    Ok(())
}

fn check_length(rules: &Map<String, Value>, length: usize, min_key: &str, max_key: &str, path: &str) -> Result<(), String> {
    let length = length as u64;
    if rules.get(min_key).and_then(Value::as_u64).is_some_and(|min| length < min) {
        return Err(format!("{path}: shorter than {min_key}"));
    }
    if rules.get(max_key).and_then(Value::as_u64).is_some_and(|max| length > max) {
        return Err(format!("{path}: longer than {max_key}"));
    }
    Ok(())
}

fn check_object(rules: &Map<String, Value>, map: &Map<String, Value>, path: &str, depth: usize) -> Result<(), String> {
    for required in rules.get("required").and_then(Value::as_array).into_iter().flatten() {
        if let Some(key) = required.as_str() {
            if !map.contains_key(key) {
                return Err(format!("{path}.{key}: required"));
            }
        }
    }
    let properties = rules.get("properties").and_then(Value::as_object);
    let closed = rules.get("additionalProperties") == Some(&Value::Bool(false));
    for (key, item) in map {
        match properties.and_then(|declared| declared.get(key)) {
            Some(item_schema) => check(item_schema, item, &format!("{path}.{key}"), depth + 1)?,
            None if closed => return Err(format!("{path}.{key}: not an allowed property")),
            None => {}
        }
    }
    Ok(())
}

/// JSON equality where `1` and `1.0` are the same number.
fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_equal(x, y)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(key, x)| b.get(key).is_some_and(|y| json_equal(x, y)))
        }
        _ => left == right,
    }
}
