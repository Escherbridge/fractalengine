//! Stored-authorized desktop SELECT requests; see src/AGENTS.md (scoped-query).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use fe_query::sql_guard::{self, GuardMode};
use fe_runtime::messages::{DbResult, EntityType};
use serde_json::Value;

use crate::repo::Db;

pub const MAX_PETALS: usize = 16;
pub const MAX_SQL_BYTES: usize = 64 * 1024;
pub const MAX_BIND_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 10_000;
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const CLIENT_TIMEOUT_SECS: u64 = sql_guard::STATEMENT_TIMEOUT_SECS + 2;
const DENIED: &str = "not found or not permitted";

/// A read plan whose stored role and ancestry snapshots are fenced at execution.
pub struct ScopedRead {
    sql: String,
    bindings: Vec<(String, Value)>,
    result_index: usize,
}

impl ScopedRead {
    /// Execute exactly the prepared transaction and strictly decode its SELECT slot.
    pub async fn execute(self, db: &Db) -> Result<Vec<Value>, String> {
        let mut query = db.query(self.sql);
        for binding in self.bindings {
            query = query.bind(binding);
        }
        let response = tokio::time::timeout(
            Duration::from_secs(sql_guard::STATEMENT_TIMEOUT_SECS + 1),
            query,
        )
        .await
        .map_err(|_| "scoped query timed out".to_string())?;
        let mut response = response
            .and_then(|response| response.check())
            .map_err(|_| DENIED.to_string())?;
        if response.num_statements() != self.result_index + 2 {
            return Err("scoped query returned an unexpected statement count".into());
        }
        let rows: Vec<Value> = response
            .take(self.result_index)
            .map_err(|_| "scoped query result could not be decoded".to_string())?;
        if rows.len() > MAX_ROWS {
            return Err(
                "row cap exceeded (limit 10000 rows; narrow the query or add LIMIT)".into(),
            );
        }
        sql_guard::enforce_byte_ceiling(&rows, MAX_RESULT_BYTES, "8 MiB")?;
        Ok(rows)
    }
}

/// Validate requested targets and capture the same stored Viewer policy that the read fences.
pub async fn prepare(
    db: &Db,
    sql: &str,
    vars: &HashMap<String, Value>,
    petals: &[String],
    local_did: &str,
) -> Result<ScopedRead, String> {
    if sql.len() > MAX_SQL_BYTES {
        return Err("query exceeds 64 KiB".into());
    }
    if petals.is_empty()
        || petals.len() > MAX_PETALS
        || petals.iter().any(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
        || petals.iter().collect::<HashSet<_>>().len() != petals.len()
    {
        return Err("invalid requested petal scope".into());
    }
    let prepared = sql_guard::prepare_scoped_sql(sql, petals, GuardMode::Query)?;
    validate_bindings(&prepared.canonical, vars)?;
    if !prepared.only_reads_petal_scoped_tables() {
        return Err("desktop queries may only read petal-scoped tables".into());
    }
    let canonical_upper = prepared.canonical.to_ascii_uppercase();
    for word in [
        "VERSE",
        "FRACTAL",
        "FIELD_DEF",
        "CRATE_ENTRY",
        "TIMEOUT",
        "PARALLEL",
        "EXPLAIN",
        "FETCH",
        "VERSION",
        "FUNCTION",
        "WITH",
        "TEMPFILES",
        "FUTURE",
    ] {
        if sql_guard::contains_word(&canonical_upper, word) {
            return Err(format!("{word} is not allowed in desktop queries"));
        }
    }
    let executable = sql_guard::with_statement_timeout(&prepared.sql);
    let parsed = surrealdb_core::syn::parse(&executable)
        .map_err(|_| "scoped SELECT does not parse".to_string())?;
    if parsed.num_statements() != 1 {
        return Err("scoped SELECT must parse as exactly one statement".into());
    }

    let mut guards = Vec::new();
    let mut bindings = Vec::new();
    for (index, petal) in petals.iter().enumerate() {
        let access = super::entity::prepare_parent_access(
            db,
            EntityType::Petal,
            petal,
            local_did,
            fe_policy::RoleLevel::Viewer,
        )
        .await
        .map_err(|_| DENIED.to_string())?;
        let (target_guards, target_bindings) = access.scoped_read_fence(index);
        guards.extend(target_guards);
        bindings.extend(target_bindings);
    }
    let result_index = guards.len() + 1;
    let script = format!(
        "BEGIN TRANSACTION;\n{};\n{executable};\nCOMMIT TRANSACTION;",
        guards.join(";\n")
    );
    let parsed_script = surrealdb_core::syn::parse(&script)
        .map_err(|_| "scoped read transaction does not parse".to_string())?;
    if parsed_script.num_statements() != result_index + 2 {
        return Err("scoped read transaction boundaries are ambiguous".into());
    }
    bindings.extend(vars.iter().map(|(key, value)| (key.clone(), value.clone())));
    Ok(ScopedRead {
        sql: script,
        bindings,
        result_index,
    })
}

/// Always echo the required request ID, including refusal and timeout paths.
pub async fn scoped_query_reply(
    db: &Db,
    sql: &str,
    vars: &HashMap<String, Value>,
    petals: &[String],
    local_did: &str,
    correlation_id: String,
) -> DbResult {
    if correlation_id.is_empty() || correlation_id.len() > 128 {
        return DbResult::QueryFailed {
            error: "invalid query request ID".into(),
            correlation_id,
        };
    }
    let outcome = tokio::time::timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS), async {
        prepare(db, sql, vars, petals, local_did)
            .await?
            .execute(db)
            .await
    })
    .await;
    match outcome {
        Ok(Ok(data)) => DbResult::QueryResult {
            data,
            correlation_id: Some(correlation_id),
        },
        Ok(Err(error)) => DbResult::QueryFailed {
            error,
            correlation_id,
        },
        Err(_) => DbResult::QueryFailed {
            error: "scoped query timed out (7s)".into(),
            correlation_id,
        },
    }
}

fn validate_bindings(sql: &str, vars: &HashMap<String, Value>) -> Result<(), String> {
    if vars.len() > 64 {
        return Err("query binding limits exceeded".into());
    }
    for key in vars.keys() {
        let lower = key.to_ascii_lowercase();
        if key.is_empty()
            || key.len() > 64
            || !key.as_bytes()[0].is_ascii_alphabetic()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || lower.starts_with("scoped_query_")
            || lower.starts_with("entity_")
            || [
                "auth", "session", "token", "this", "parent", "value", "before", "after", "access",
            ]
            .contains(&lower.as_str())
        {
            return Err("reserved or invalid query binding name".into());
        }
    }
    let mut pending: Vec<(&Value, usize)> = vars.values().map(|value| (value, 0)).collect();
    let mut values_seen = 0usize;
    let mut string_bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        values_seen += 1;
        if depth > 16 || values_seen > 8192 {
            return Err("query binding nesting/value limit exceeded".into());
        }
        match value {
            Value::Array(values) => {
                if values.len() + pending.len() > 8192 {
                    return Err("query binding value limit exceeded".into());
                }
                pending.extend(values.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(values) => {
                if values.len() + pending.len() > 8192 {
                    return Err("query binding value limit exceeded".into());
                }
                string_bytes = values
                    .keys()
                    .fold(string_bytes, |total, key| total.saturating_add(key.len()));
                pending.extend(values.values().map(|v| (v, depth + 1)));
            }
            Value::String(value) => string_bytes = string_bytes.saturating_add(value.len()),
            _ => {}
        }
        if string_bytes > MAX_BIND_BYTES {
            return Err("query binding byte limit exceeded".into());
        }
    }
    if serde_json::to_vec(vars)
        .map_err(|_| "invalid query bindings")?
        .len()
        > MAX_BIND_BYTES
    {
        return Err("query binding byte limit exceeded".into());
    }
    validate_expression_tokens(sql, vars)
}

/// Allow only declared parameters and the small data-function vocabulary used by desktop reads.
fn validate_expression_tokens(sql: &str, vars: &HashMap<String, Value>) -> Result<(), String> {
    const FUNCTIONS: &[&str] = &[
        "count",
        "array::len",
        "math::min",
        "math::max",
        "math::mean",
        "math::sum",
        "math::median",
        "math::stddev",
        "math::variance",
        "math::spread",
        "math::product",
        "math::round",
        "math::floor",
        "math::ceil",
        "math::abs",
        "math::sqrt",
        "math::pow",
    ];
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '\'' | '"') {
            let quote = chars[i];
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                } else if chars[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if (chars[i] == '-' && chars.get(i + 1) == Some(&'>'))
            || (chars[i] == '<' && chars.get(i + 1) == Some(&'-'))
        {
            return Err("graph traversal is not allowed in desktop queries".into());
        }
        if !chars[i].is_ascii()
            || matches!(chars[i], '/' | '\\' | '|')
            || (chars[i] == '.' && chars.get(i + 1) == Some(&'.'))
        {
            return Err("regex, range, closure and non-ASCII token syntax is not allowed in desktop queries".into());
        }
        if matches!(chars[i], ')' | ']' | '}') {
            let mut next = i + 1;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if chars.get(next) == Some(&'(') {
                return Err("dynamic calls are not allowed in desktop queries".into());
            }
        }
        if chars[i] == '$' {
            i += 1;
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            if name.is_empty() || !vars.contains_key(&name) {
                return Err("query references an undeclared or reserved binding".into());
            }
            let mut next = i;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if chars.get(next) == Some(&'(') {
                return Err("dynamic calls are not allowed in desktop queries".into());
            }
            continue;
        }
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let mut name: String = chars[start..i].iter().collect();
            loop {
                let mut next = i;
                while next < chars.len() && chars[next].is_whitespace() {
                    next += 1;
                }
                if chars.get(next) != Some(&':') || chars.get(next + 1) != Some(&':') {
                    break;
                }
                i = next + 2;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                let segment = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if i == segment {
                    return Err("invalid function path".into());
                }
                name.push_str("::");
                name.extend(chars[segment..i].iter());
            }
            let mut next = i;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if chars.get(next) == Some(&'(') {
                let lower = name.to_ascii_lowercase();
                if !["select", "from", "where", "in", "not", "and", "or"].contains(&lower.as_str())
                    && !FUNCTIONS.contains(&lower.as_str())
                {
                    return Err("function is not allowed in desktop queries".into());
                }
            }
            continue;
        }
        i += 1;
    }
    Ok(())
}
