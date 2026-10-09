//! Shared guard pipeline for /query + export/share egress — see `fe-api/AGENTS.md` §query-guard.
//!
//! One entry point per surface (`guard_and_prepare_query[_with_mode]`,
//! `prepare_scoped_sql`) so export/share handlers cannot bypass any guard
//! `execute_query` enforces. Row scoping is FROM-substitution (M4 fix B1).

use std::sync::Arc;

use crate::server::ApiState;

/// SQL that has passed every static guard, with the scope filter applied.
pub struct GuardedQuery {
    pub sql: String,
}

/// Keywords rejected anywhere in a read-only query (whole-word match).
const BLOCKED_KEYWORDS: &[&str] = &[
    "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT", "LET", "RETURN", "INFO",
    "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL", "IF", "BEGIN", "COMMIT", "CANCEL",
];

/// Tables a read-only query may target. ROLE/VERSE_MEMBER deliberately
/// excluded — RBAC data is not readable via the BI egress path (2026-07-15
/// security review; the role-gated elevated endpoint retains them).
const ALLOWED_TABLES: &[&str] = &[
    "NODE",
    "VERSE",
    "FRACTAL",
    "PETAL",
    "NODE_LOG",
    "FIELD_DEF",
    "ASSET",
    "MODEL",
    "ROOM",
    "CRATE_REGISTRY",
    "CRATE_ENTRY",
    "IOT_READING",
];

/// Tables whose rows carry `petal_id` and are scope-filtered at the source.
const PETAL_SCOPED_TABLES: &[&str] = &["NODE", "IOT_READING"];

/// SELECT clauses that may follow a FROM target (SurrealQL 3 `parse_select_stmt`
/// order). Anything else after a target (an operator) would extend the target
/// EXPRESSION — `FROM verse AND role` evaluates to the `role` table.
const FROM_BOUNDARY_KEYWORDS: &[&str] = &[
    "WITH",
    "WHERE",
    "SPLIT",
    "GROUP",
    "ORDER",
    "LIMIT",
    "START",
    "FETCH",
    "VERSION",
    "TIMEOUT",
    "TEMPFILES",
    "EXPLAIN",
    "PARALLEL",
];

/// DB-side statement timeout appended to every guarded SELECT (M4 fix M2).
pub const STATEMENT_TIMEOUT_SECS: u64 = 5;

/// Client-side backstop; deliberately longer than the DB-side TIMEOUT so the
/// DB aborts (and frees its thread) first.
const CLIENT_TIMEOUT_SECS: u64 = STATEMENT_TIMEOUT_SECS + 1;

const UNSUPPORTED_FROM: &str =
    "unsupported FROM target (only table names or subqueries are allowed)";

/// Which egress surface a guarded query serves (dialect strictness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardMode {
    /// `/api/v1/query` + MCP `query`: subqueries allowed — every FROM is scoped.
    Query,
    /// JSON share links: one flat SELECT, no comments.
    Egress,
    /// Parquet/CSV export + share: `Egress` + projection forced to `*`.
    Export,
}

/// Sliding 1s-window rate limit keyed by caller; `label` names the limit in the error.
pub async fn check_rate_limit(
    state: &ApiState,
    key: &str,
    max_per_sec: u32,
    label: &str,
) -> Result<(), String> {
    let now = std::time::Instant::now();
    let mut limiter = state.query_rate_limiter.lock().await;

    // Evict stale entries older than 10s to prevent unbounded growth.
    limiter.retain(|_, (_, ts)| now.duration_since(*ts) < std::time::Duration::from_secs(10));

    let entry = limiter.entry(key.to_string()).or_insert((0u32, now));
    if now.duration_since(entry.1) > std::time::Duration::from_secs(1) {
        // Reset window
        *entry = (1, now);
    } else {
        entry.0 += 1;
        if entry.0 > max_per_sec {
            return Err(format!("rate limit exceeded ({label})"));
        }
    }
    Ok(())
}

/// Static single-SELECT validation: semicolon, SELECT-only, keyword blocklist, table whitelist.
pub fn validate_select_sql(sql: &str) -> Result<(), String> {
    let sql_trimmed = sql.trim();

    // Reject semicolons — prevents multi-statement chaining.
    if sql_trimmed.contains(';') {
        return Err("semicolons are not allowed (single statement only)".to_string());
    }

    // ASCII uppercase keeps byte offsets identical to the input.
    let sql_upper = sql_trimmed.to_ascii_uppercase();
    if !sql_upper.starts_with("SELECT") {
        return Err("only SELECT statements are allowed".to_string());
    }

    // Reject any dangerous keyword anywhere in the statement (including subqueries,
    // comments, RETURN wrappers, LET bindings, etc.).
    for keyword in BLOCKED_KEYWORDS {
        if contains_word(&sql_upper, keyword) {
            return Err(format!("{keyword} keyword is not allowed in queries"));
        }
    }

    // Table whitelist: EVERY FROM clause (top level and subqueries) must
    // target whitelisted tables — see AGENTS.md §query-guard (subquery bypass).
    for target in from_clause_targets(&sql_upper)? {
        if !ALLOWED_TABLES.contains(&target.table.as_str()) {
            return Err(format!(
                "queries against table '{}' are not allowed",
                target.table
            ));
        }
    }

    Ok(())
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// True if `word` appears as a whole word (not inside a longer identifier).
pub fn contains_word(sql_upper: &str, word: &str) -> bool {
    !word_positions(sql_upper, word).is_empty()
}

/// Byte offsets of every whole-word occurrence of `word` (strings and
/// comments included — deliberately lexer-free, so never under-reports).
fn word_positions(sql_upper: &str, word: &str) -> Vec<usize> {
    let bytes = sql_upper.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(pos) = sql_upper[start..].find(word) {
        let abs = start + pos;
        let before_ok = abs == 0 || !is_ident(bytes[abs - 1]);
        let after = abs + word.len();
        let after_ok = after >= bytes.len() || !is_ident(bytes[after]);
        if before_ok && after_ok {
            out.push(abs);
        }
        start = after;
    }
    out
}

/// True if the whole word `word` starts at byte `i`.
fn word_at(bytes: &[u8], i: usize, word: &str) -> bool {
    bytes[i.min(bytes.len())..].starts_with(word.as_bytes())
        && bytes.get(i + word.len()).is_none_or(|b| !is_ident(*b))
}

fn skip_ascii_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Extract the (uppercased) table name following the first whole-word FROM, if any.
pub fn from_table(sql_upper: &str) -> Option<String> {
    let from_pos = *word_positions(sql_upper, "FROM").first()?;
    let after_from = sql_upper[from_pos + 4..].trim_start();
    let table_name: String = after_from
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    Some(table_name)
}

/// One table identifier in a FROM clause, with its byte span in the scanned SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FromTarget {
    pub table: String,
    pub start: usize,
    pub end: usize,
}

/// Every table identifier targeted by any FROM clause; errors on FROM
/// targets that cannot be whitelist-checked (variables, literals, record ids,
/// operator expressions like `FROM verse AND role`, non-SELECT parens).
pub fn from_clause_tables(sql_upper: &str) -> Result<Vec<String>, String> {
    Ok(from_clause_targets(sql_upper)?
        .into_iter()
        .map(|t| t.table)
        .collect())
}

/// [`from_clause_tables`] with byte spans (the scope rewrite's anchor points).
pub fn from_clause_targets(sql_upper: &str) -> Result<Vec<FromTarget>, String> {
    let bytes = sql_upper.as_bytes();
    let mut targets = Vec::new();
    for from_pos in word_positions(sql_upper, "FROM") {
        let mut i = from_pos + 4;
        loop {
            i = skip_ascii_ws(bytes, i);
            if bytes.get(i) == Some(&b'(') {
                // A parenthesized source must be a SELECT; its own FROM is
                // scanned by the outer loop.
                if !word_at(bytes, skip_ascii_ws(bytes, i + 1), "SELECT") {
                    return Err(UNSUPPORTED_FROM.to_string());
                }
                i = matching_paren(bytes, i).ok_or_else(|| UNSUPPORTED_FROM.to_string())? + 1;
            } else {
                let start = i;
                while i < bytes.len() && is_ident(bytes[i]) {
                    i += 1;
                }
                if i == start {
                    return Err(UNSUPPORTED_FROM.to_string());
                }
                targets.push(FromTarget {
                    table: sql_upper[start..i].to_string(),
                    start,
                    end: i,
                });
            }
            let next = skip_ascii_ws(bytes, i);
            if bytes.get(next) == Some(&b',') {
                i = next + 1;
                continue;
            }
            let clean = next >= bytes.len()
                || bytes[next] == b')'
                || FROM_BOUNDARY_KEYWORDS
                    .iter()
                    .any(|kw| word_at(bytes, next, kw));
            if !clean {
                return Err(UNSUPPORTED_FROM.to_string());
            }
            break;
        }
    }
    Ok(targets)
}

/// Index of the `)` closing the `(` at `open`, skipping SurrealQL strings and
/// backtick identifiers (backslash escapes). Fails closed (`None`) on
/// comments, `⟨⟩` identifiers, or imbalance.
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'\'' | b'"' | b'`') => {
                i += 1;
                loop {
                    match *bytes.get(i)? {
                        b'\\' => i += 2,
                        b if b == q => break,
                        _ => i += 1,
                    }
                }
            }
            b'#' => return None,
            b'-' if bytes.get(i + 1) == Some(&b'-') => return None,
            b'/' if matches!(bytes.get(i + 1), Some(b'/' | b'*')) => return None,
            // UTF-8 lead bytes of ⟨ (E2 9F A8).
            0xE2 if bytes.get(i + 1) == Some(&0x9F) => return None,
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Elevated-query validation: whole-word DDL/system keyword ban plus
/// all-occurrence FROM/INTO/UPDATE target whitelisting (no substring or
/// first-occurrence-only bypasses).
pub fn validate_elevated_sql(sql_upper: &str, allowed_tables: &[&str]) -> Result<(), String> {
    for keyword in ["DEFINE", "REMOVE", "INFO", "SLEEP", "KILL"] {
        if contains_word(sql_upper, keyword) {
            return Err(format!("{keyword} is not allowed in elevated queries"));
        }
    }
    let bytes = sql_upper.as_bytes();
    for keyword in ["FROM", "INTO", "UPDATE"] {
        let mut start = 0;
        while let Some(pos) = sql_upper[start..].find(keyword) {
            let abs_pos = start + pos;
            let before_ok = abs_pos == 0 || !is_ident(bytes[abs_pos - 1]);
            let after_kw = abs_pos + keyword.len();
            let after_ok = after_kw >= bytes.len() || !is_ident(bytes[after_kw]);
            start = after_kw;
            if !(before_ok && after_ok) {
                continue;
            }
            let rest = sql_upper[after_kw..].trim_start();
            if rest.starts_with('(') {
                continue;
            }
            let table: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !table.is_empty() && !allowed_tables.contains(&table.as_str()) {
                return Err(format!(
                    "table '{table}' is not allowed in elevated queries"
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scoped dialect + FROM-substitution (M4 fix B1 — AGENTS.md §query-guard)
// ---------------------------------------------------------------------------

/// Collapse whitespace runs to one space OUTSIDE strings, escaped
/// identifiers and comments (their bytes are copied verbatim). Semantics-only:
/// no guard decision depends on this lexer being exact.
pub fn normalize_whitespace(sql: &str) -> String {
    let chars: Vec<char> = sql.trim().chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let mut pending_space = false;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            pending_space = true;
            i += 1;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        let next = chars.get(i + 1).copied();
        let end = match c {
            '\'' | '"' | '`' | '⟨' => {
                let close = if c == '⟨' { '⟩' } else { c };
                let mut j = i + 1;
                while j < chars.len() && chars[j] != close {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                (j + 1).min(chars.len())
            }
            '#' => line_end(&chars, i),
            '-' if next == Some('-') => line_end(&chars, i),
            '/' if next == Some('/') => line_end(&chars, i),
            '/' if next == Some('*') => {
                let mut j = i + 2;
                while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                    j += 1;
                }
                (j + 2).min(chars.len())
            }
            _ => i + 1,
        };
        out.extend(&chars[i..end]);
        i = end;
    }
    out
}

/// Index just past the newline ending a single-line comment starting at `i`.
fn line_end(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while j < chars.len() && chars[j] != '\n' {
        j += 1;
    }
    (j + 1).min(chars.len())
}

/// Export/share: reject comment tokens anywhere (string literals included).
fn reject_comments(sql: &str) -> Result<(), String> {
    for token in ["--", "#", "//", "/*"] {
        if sql.contains(token) {
            return Err(format!(
                "comments ('{token}') are not allowed in export/share queries"
            ));
        }
    }
    Ok(())
}

/// Reject every way to name a record/table WITHOUT the table's name appearing
/// as a plain word (escaped identifiers, record constructors, `r"..."`).
fn reject_record_constructors(canonical: &str, upper: &str) -> Result<(), String> {
    if canonical.contains('`') || canonical.contains('⟨') || canonical.contains('⟩') {
        return Err("escaped identifiers (`...` / ⟨...⟩) are not allowed in scoped queries".into());
    }
    let compact: String = upper.chars().filter(|c| !c.is_whitespace()).collect();
    for needle in [
        "TYPE::THING",
        "TYPE::RECORD",
        "TYPE::TABLE",
        "RECORD::",
        "<RECORD",
    ] {
        if compact.contains(needle) {
            return Err(format!(
                "record/table constructors ({}) are not allowed in scoped queries",
                needle.to_ascii_lowercase()
            ));
        }
    }
    let bytes = upper.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'R'
            && matches!(bytes[i + 1], b'\'' | b'"')
            && (i == 0 || !is_ident(bytes[i - 1]))
        {
            return Err(
                "record-id string literals (r\"...\") are not allowed in scoped queries".into(),
            );
        }
    }
    Ok(())
}

/// The server-built row predicate for a petal allow-list (`false` = deny all).
/// Ids outside `[A-Za-z0-9_-]` are dropped, never escaped.
fn petal_filter_clause(petals: &[String]) -> String {
    let safe: Vec<&str> = petals
        .iter()
        .map(String::as_str)
        .filter(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .collect();
    match safe.as_slice() {
        [] => "false".to_string(),
        [one] => format!("petal_id = '{one}'"),
        many => format!(
            "petal_id IN [{}]",
            many.iter()
                .map(|p| format!("'{p}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Output of [`prepare_scoped_sql`].
#[derive(Debug)]
pub struct PreparedSql {
    /// Whitespace-normalized SQL as written (pre-rewrite).
    pub canonical: String,
    /// Uppercased FROM-target table names in source order.
    pub tables: Vec<String>,
    /// Executable SQL: every node/iot_reading FROM target replaced by a
    /// server-built petal-filtered subquery.
    pub sql: String,
}

impl PreparedSql {
    /// True if any FROM target is a petal-scoped table (needs a scope lookup).
    pub fn reads_petal_scoped_table(&self) -> bool {
        self.tables
            .iter()
            .any(|t| PETAL_SCOPED_TABLES.contains(&t.as_str()))
    }
}

/// Normalize → validate → scoped-dialect checks → FROM-substitution.
///
/// `FROM node` becomes `FROM (SELECT * FROM node WHERE <petal filter>)`, so
/// the user's WHERE/OR/projection only ever sees already-scoped rows — see
/// AGENTS.md §query-guard for the soundness argument.
pub fn prepare_scoped_sql(
    sql: &str,
    petals: &[String],
    mode: GuardMode,
) -> Result<PreparedSql, String> {
    let trimmed = sql.trim();
    if mode != GuardMode::Query {
        reject_comments(trimmed)?;
    }
    let canonical = normalize_whitespace(trimmed);
    validate_select_sql(&canonical)?;
    let upper = canonical.to_ascii_uppercase();
    reject_record_constructors(&canonical, &upper)?;
    let targets = from_clause_targets(&upper)?;

    if mode != GuardMode::Query {
        if word_positions(&upper, "SELECT").len() != 1 {
            return Err("nested SELECT is not allowed in export/share queries".into());
        }
        if word_positions(&upper, "FROM").len() != 1 || targets.len() != 1 {
            return Err("export/share queries must read exactly one table".into());
        }
    }

    // Every mention of a petal-scoped table must be a FROM target we rewrite.
    for table in PETAL_SCOPED_TABLES {
        for pos in word_positions(&upper, table) {
            if !targets.iter().any(|t| t.start == pos) {
                return Err(format!(
                    "'{}' may only appear as a FROM table name in scoped queries",
                    table.to_ascii_lowercase()
                ));
            }
        }
    }

    let filter = petal_filter_clause(petals);
    let mut out = canonical.clone();
    for t in targets.iter().rev() {
        if PETAL_SCOPED_TABLES.contains(&t.table.as_str()) {
            let original = canonical[t.start..t.end].to_string();
            out.replace_range(
                t.start..t.end,
                &format!("(SELECT * FROM {original} WHERE {filter})"),
            );
        }
    }
    if mode == GuardMode::Export {
        // Export rows map onto a fixed shape: the projection is irrelevant,
        // and `*` guarantees the real `petal_id` column reaches the post-filter.
        let from_pos = word_positions(&upper, "FROM")[0];
        out.replace_range("SELECT".len()..from_pos, " * ");
    }

    Ok(PreparedSql {
        canonical,
        tables: targets.into_iter().map(|t| t.table).collect(),
        sql: out,
    })
}

/// Petal allow-list for a token scope: petal → itself; fractal/verse → every
/// petal under it (resolved via petal.fractal_id → fractal.verse_id);
/// unparseable → empty (deny all).
pub async fn resolve_scope_petals(state: &ApiState, scope: &str) -> Result<Vec<String>, String> {
    let Ok(parts) = fe_database::parse_scope(scope) else {
        return Ok(Vec::new());
    };
    if let Some(petal_id) = parts.petal_id {
        return Ok(vec![petal_id]);
    }
    let mut vars = std::collections::HashMap::new();
    vars.insert("vid".to_string(), serde_json::json!(parts.verse_id));
    let sql = match parts.fractal_id {
        Some(fractal_id) => {
            vars.insert("fid".to_string(), serde_json::json!(fractal_id));
            "SELECT VALUE petal_id FROM petal WHERE fractal_id IN \
             (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $vid AND fractal_id = $fid)"
        }
        None => {
            "SELECT VALUE petal_id FROM petal WHERE fractal_id IN \
             (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $vid)"
        }
    };
    let rows = run_guarded_query_via_state(
        state,
        &GuardedQuery {
            sql: sql.to_string(),
        },
        &vars,
        crate::limits::QUERY_ROW_CAP,
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect())
}

/// Full guard chain for `/api/v1/query` (+ MCP `query`): [`GuardMode::Query`].
pub async fn guard_and_prepare_query(
    state: &ApiState,
    rate_key: &str,
    rate_max_per_sec: u32,
    rate_label: &str,
    scope: &str,
    sql: &str,
) -> Result<GuardedQuery, String> {
    guard_and_prepare_query_with_mode(
        state,
        rate_key,
        rate_max_per_sec,
        rate_label,
        scope,
        sql,
        GuardMode::Query,
    )
    .await
}

/// Rate limit → static dialect checks → scope lookup (only when a
/// petal-scoped table is read) → FROM-substitution.
pub async fn guard_and_prepare_query_with_mode(
    state: &ApiState,
    rate_key: &str,
    rate_max_per_sec: u32,
    rate_label: &str,
    scope: &str,
    sql: &str,
    mode: GuardMode,
) -> Result<GuardedQuery, String> {
    check_rate_limit(state, rate_key, rate_max_per_sec, rate_label).await?;
    let static_pass = prepare_scoped_sql(sql, &[], mode)?;
    if !static_pass.reads_petal_scoped_table() {
        return Ok(GuardedQuery {
            sql: static_pass.sql,
        });
    }
    let petals = resolve_scope_petals(state, scope).await?;
    Ok(GuardedQuery {
        sql: prepare_scoped_sql(sql, &petals, mode)?.sql,
    })
}

/// Append the DB-side statement timeout (SurrealQL orders TIMEOUT after
/// FETCH/VERSION). The leading NEWLINE ends any trailing `--`/`#`/`//`
/// comment (allowed in Query mode) so it cannot swallow the clause.
pub fn with_statement_timeout(sql: &str) -> String {
    format!("{}\nTIMEOUT {STATEMENT_TIMEOUT_SECS}s", sql.trim_end())
}

/// Execute a guarded query with a DB-side `TIMEOUT 5s`, a client backstop, and a row cap.
pub async fn run_guarded_query(
    db: &Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>,
    guarded: &GuardedQuery,
    vars: &std::collections::HashMap<String, serde_json::Value>,
    row_cap: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let mut query_builder = db.query(with_statement_timeout(&guarded.sql));
    for (key, value) in vars {
        query_builder = query_builder.bind((key.clone(), value.clone()));
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(CLIENT_TIMEOUT_SECS), async {
        query_builder.await
    })
    .await;

    match result {
        Ok(Ok(response)) => {
            // A failed statement (incl. a DB-side TIMEOUT) must never read
            // as an empty-but-successful result.
            let mut response = response.check().map_err(|e| format!("query failed: {e}"))?;
            let mut data = Vec::new();
            let num = response.num_statements();
            for idx in 0..num {
                match response.take::<Vec<serde_json::Value>>(idx) {
                    Ok(rows) => data.extend(rows),
                    Err(_) => break,
                }
            }
            if data.len() > row_cap {
                return Err(format!(
                    "row cap exceeded (limit {row_cap} rows; narrow the query or add LIMIT)"
                ));
            }
            Ok(data)
        }
        Ok(Err(e)) => Err(format!("query failed: {e}")),
        Err(_) => Err(format!("query timed out ({CLIENT_TIMEOUT_SECS}s)")),
    }
}

/// Execute a guarded query, preferring the direct `db_reader` and falling
/// back to the `DbCommand::RawQuery` gateway channel when no direct reader is
/// wired (F10: the Windows SurrealKV per-handle lock leaves `db_reader`
/// `None` on the deployment platform). The channel request is CORRELATED (M4
/// fix B2) and mirrors the direct path's TIMEOUT + row cap exactly.
pub async fn run_guarded_query_via_state(
    state: &ApiState,
    guarded: &GuardedQuery,
    vars: &std::collections::HashMap<String, serde_json::Value>,
    row_cap: usize,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(ref db) = state.db_reader {
        return run_guarded_query(db, guarded, vars, row_cap).await;
    }
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(fe_runtime::messages::ApiCommand::DbRequest {
            cmd: fe_runtime::messages::DbCommand::RawQuery {
                sql: with_statement_timeout(&guarded.sql),
                vars: vars.clone(),
                correlation_id: Some(correlation_id.clone()),
            },
            reply_tx,
        })
        .map_err(|_| "internal channel closed".to_string())?;
    use fe_runtime::messages::DbResult;
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(CLIENT_TIMEOUT_SECS),
        reply_rx,
    )
    .await;
    let data = match reply {
        Ok(Ok(DbResult::QueryResult {
            data,
            correlation_id: Some(echoed),
        })) if echoed == correlation_id => data,
        Ok(Ok(DbResult::QueryFailed {
            error,
            correlation_id: echoed,
        })) if echoed == correlation_id => return Err(format!("query failed: {error}")),
        Ok(Ok(DbResult::Error(e))) => return Err(format!("query failed: {e}")),
        // Defense in depth over the router: anything else is never our rows.
        Ok(Ok(_)) => return Err("query failed: uncorrelated reply".to_string()),
        Ok(Err(_)) => return Err("request cancelled".to_string()),
        Err(_) => return Err(format!("query timed out ({CLIENT_TIMEOUT_SECS}s)")),
    };
    if data.len() > row_cap {
        return Err(format!(
            "row cap exceeded (limit {row_cap} rows; narrow the query or add LIMIT)"
        ));
    }
    Ok(data)
}

/// Reject a result set whose serialized JSON exceeds `max_bytes`.
pub fn enforce_byte_ceiling(
    rows: &[serde_json::Value],
    max_bytes: usize,
    label: &str,
) -> Result<(), String> {
    let mut total = 0usize;
    for row in rows {
        total += row.to_string().len() + 1;
        if total > max_bytes {
            return Err(format!(
                "result size exceeds ceiling ({label}); narrow the query"
            ));
        }
    }
    Ok(())
}

/// True for guard/DB error strings that mean a timeout (either layer).
pub fn is_timeout_error(e: &str) -> bool {
    let lower = e.to_ascii_lowercase();
    lower.contains("timed out") || lower.contains("timeout")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn p(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn semicolon_rejected() {
        assert_eq!(
            validate_select_sql("SELECT * FROM node; DELETE node").unwrap_err(),
            "semicolons are not allowed (single statement only)"
        );
    }

    #[test]
    fn non_select_rejected() {
        assert_eq!(
            validate_select_sql("DELETE FROM node").unwrap_err(),
            "only SELECT statements are allowed"
        );
    }

    #[test]
    fn blocked_keyword_rejected_whole_word_only() {
        assert!(validate_select_sql("SELECT * FROM node WHERE x = (UPDATE node)").is_err());
        // Substring of an identifier must NOT match.
        assert!(validate_select_sql("SELECT deleted FROM node").is_ok());
    }

    #[test]
    fn off_whitelist_table_rejected() {
        let err = validate_select_sql("SELECT * FROM secrets").unwrap_err();
        assert!(
            err.contains("'SECRETS'") && err.contains("not allowed"),
            "{err}"
        );
    }

    #[test]
    fn whitelisted_table_allowed() {
        assert!(validate_select_sql("SELECT * FROM node WHERE petal_id = 'p1'").is_ok());
        assert!(
            validate_select_sql("SELECT * FROM iot_reading WHERE metric = 'temperature_c'").is_ok()
        );
    }

    #[test]
    fn from_table_extracts_first_whole_word_target() {
        assert_eq!(
            from_table("SELECT * FROM NODE WHERE X = 1").as_deref(),
            Some("NODE")
        );
        assert_eq!(
            from_table("SELECT FROMAGE FROM IOT_READING").as_deref(),
            Some("IOT_READING")
        );
        assert_eq!(from_table("SELECT 1").as_deref(), None);
    }

    #[test]
    fn subquery_tables_are_whitelist_checked() {
        // 2026-07-15 security review: a nested SELECT must not reach
        // non-whitelisted tables through its own FROM clause.
        let err =
            validate_select_sql("SELECT * FROM node WHERE id IN (SELECT id FROM session_cache)")
                .unwrap_err();
        assert!(err.contains("SESSION_CACHE"), "{err}");
        assert!(validate_select_sql(
            "SELECT * FROM node WHERE id IN (SELECT anchor_node_id FROM iot_reading)"
        )
        .is_ok());
        assert!(validate_select_sql("SELECT * FROM (SELECT * FROM node)").is_ok());
    }

    #[test]
    fn multi_table_from_lists_are_fully_checked() {
        assert!(validate_select_sql("SELECT * FROM node, secrets").is_err());
        assert!(validate_select_sql("SELECT * FROM node, petal").is_ok());
    }

    /// FROM targets are SurrealQL EXPRESSIONS: an operator after a target
    /// (`verse AND role` evaluates to the `role` table) or a non-SELECT
    /// parenthesized source would reach tables the whitelist never saw.
    #[test]
    fn from_target_expressions_cannot_smuggle_tables() {
        for sql in [
            "SELECT * FROM $tbl",
            "SELECT * FROM type::table($t)",
            "SELECT * FROM verse AND role",
            "SELECT * FROM verse OR role",
            "SELECT * FROM (role)",
            "SELECT * FROM (SELECT * FROM verse) AND role",
            "SELECT * FROM (SELECT * FROM verse WHERE n = ') WHERE ') AND role",
            "SELECT * FROM node:abc",
            "SELECT * FROM ONLY node",
            "SELECT * FROM `role`",
        ] {
            assert!(validate_select_sql(sql).is_err(), "should reject: {sql}");
        }
        assert!(validate_select_sql("SELECT * FROM (SELECT * FROM node) WHERE x = 1").is_ok());
        assert!(validate_select_sql("SELECT * FROM node ORDER BY x LIMIT 5").is_ok());
    }

    #[test]
    fn rbac_tables_not_readable_via_egress() {
        assert!(validate_select_sql("SELECT * FROM role").is_err());
        assert!(validate_select_sql("SELECT * FROM verse_member").is_err());
    }

    #[test]
    fn byte_ceiling_enforced() {
        let rows: Vec<serde_json::Value> = (0..10)
            .map(|i| serde_json::json!({ "k": format!("row-{i}") }))
            .collect();
        assert!(enforce_byte_ceiling(&rows, 1024, "1 KiB").is_ok());
        let err = enforce_byte_ceiling(&rows, 32, "32 B").unwrap_err();
        assert!(err.contains("result size exceeds ceiling (32 B)"), "{err}");
    }

    #[test]
    fn whitespace_normalized_outside_literals_only() {
        assert_eq!(
            normalize_whitespace("SELECT  *\n\tFROM   node WHERE n = 'a  b'"),
            "SELECT * FROM node WHERE n = 'a  b'"
        );
        // A `--` comment keeps its newline so normalization never comments
        // out the following line.
        assert_eq!(
            normalize_whitespace("SELECT * -- c\nFROM node"),
            "SELECT * -- c\nFROM node"
        );
    }

    /// Source substitution: the user's text never sits inside the filtered
    /// subquery, so OR-precedence/whitespace/projection tricks see only
    /// scoped rows.
    #[test]
    fn scoped_rewrite_substitutes_every_petal_scoped_from_target() {
        let out = prepare_scoped_sql(
            "SELECT *  FROM\n node WHERE true OR true",
            &p(&["P1"]),
            GuardMode::Query,
        )
        .unwrap();
        assert_eq!(
            out.sql,
            "SELECT * FROM (SELECT * FROM node WHERE petal_id = 'P1') WHERE true OR true"
        );

        let out = prepare_scoped_sql(
            "SELECT *, (SELECT * FROM iot_reading) AS x FROM node",
            &p(&["P1", "P2"]),
            GuardMode::Query,
        )
        .unwrap();
        assert_eq!(
            out.sql,
            "SELECT *, (SELECT * FROM (SELECT * FROM iot_reading WHERE petal_id IN ['P1', 'P2'])) \
             AS x FROM (SELECT * FROM node WHERE petal_id IN ['P1', 'P2'])"
        );

        // Empty allow-list = deny all; unsafe ids are dropped, never escaped.
        let out = prepare_scoped_sql("SELECT * FROM node", &p(&["a'b", "x\\"]), GuardMode::Query)
            .unwrap();
        assert_eq!(out.sql, "SELECT * FROM (SELECT * FROM node WHERE false)");

        // Non-petal tables are untouched.
        let out = prepare_scoped_sql("SELECT * FROM verse", &p(&["P1"]), GuardMode::Query).unwrap();
        assert_eq!(out.sql, "SELECT * FROM verse");
    }

    #[test]
    fn scoped_dialect_rejects_unscoped_record_access() {
        for sql in [
            "SELECT *, node:abc.* AS leak FROM verse",
            "SELECT * FROM verse WHERE (node:abc.position) != NONE",
            "SELECT type::thing('no' + 'de', 'x').* FROM verse",
            "SELECT type :: record('node', 'x') FROM verse",
            "SELECT <record> 'node:x' FROM verse",
            "SELECT r\"no\" FROM verse",
            "SELECT `a` FROM node",
            "SELECT * FROM verse WHERE name = 'node'",
        ] {
            assert!(
                prepare_scoped_sql(sql, &p(&["P1"]), GuardMode::Query).is_err(),
                "should reject: {sql}"
            );
        }
    }

    #[test]
    fn egress_modes_reject_comments_and_nesting() {
        for sql in [
            "SELECT * FROM node -- x",
            "SELECT * FROM node /* x */",
            "SELECT * FROM node # x",
            "SELECT * FROM node WHERE u = 'http://x'",
            "SELECT *, (SELECT * FROM iot_reading) AS x FROM node",
            "SELECT * FROM node WHERE node_id IN (SELECT VALUE node_id FROM node)",
            "SELECT * FROM node, petal",
        ] {
            for mode in [GuardMode::Egress, GuardMode::Export] {
                assert!(
                    prepare_scoped_sql(sql, &p(&["P1"]), mode).is_err(),
                    "{mode:?} should reject: {sql}"
                );
            }
        }
        // Query mode keeps (scoped) subqueries.
        assert!(prepare_scoped_sql(
            "SELECT *, (SELECT * FROM iot_reading) AS x FROM node",
            &p(&["P1"]),
            GuardMode::Query
        )
        .is_ok());
    }

    #[test]
    fn export_mode_forces_star_projection() {
        let out = prepare_scoped_sql(
            "SELECT node_id, 'P1' AS petal_id FROM node WHERE x = 1",
            &p(&["P1"]),
            GuardMode::Export,
        )
        .unwrap();
        assert_eq!(
            out.sql,
            "SELECT * FROM (SELECT * FROM node WHERE petal_id = 'P1') WHERE x = 1"
        );
        assert_eq!(out.tables, vec!["NODE".to_string()]);
    }

    #[test]
    fn statement_timeout_is_appended_last_and_survives_trailing_comments() {
        assert_eq!(
            with_statement_timeout("SELECT * FROM node ORDER BY x LIMIT 3 "),
            "SELECT * FROM node ORDER BY x LIMIT 3\nTIMEOUT 5s"
        );
        // A trailing line comment ends at the newline — TIMEOUT stays live.
        assert!(with_statement_timeout("SELECT * FROM node -- x").ends_with("-- x\nTIMEOUT 5s"));
    }

    #[test]
    fn elevated_ddl_ban_survives_whitespace_tricks() {
        const TABLES: &[&str] = &["NODE", "ROLE"];
        // Multi-word substring match was bypassable with doubled whitespace.
        assert!(validate_elevated_sql("DEFINE  TABLE X", TABLES).is_err());
        assert!(validate_elevated_sql("DEFINE\nTABLE X", TABLES).is_err());
        assert!(validate_elevated_sql("INFO FOR DB", TABLES).is_err());
        assert!(validate_elevated_sql("UPDATE NODE SET X = 1", TABLES).is_ok());
    }

    #[test]
    fn elevated_checks_every_target_occurrence() {
        const TABLES: &[&str] = &["NODE", "ROLE"];
        // First-occurrence-only checking missed later FROM/UPDATE targets.
        assert!(
            validate_elevated_sql("UPDATE NODE SET X = (SELECT Y FROM SESSION_CACHE)", TABLES)
                .is_err()
        );
        assert!(validate_elevated_sql("UPDATE NODE SET X = (SELECT Y FROM ROLE)", TABLES).is_ok());
    }
}
