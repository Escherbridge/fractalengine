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
/// security review; the elevated endpoint excludes them too — `ELEVATED_TABLES`).
/// `NODE_LOG` deliberately excluded (DEC-C19 N1, 2026-10-09): it has no
/// documented `/query`/export/share consumer and its `payload` carries a
/// node's `petal_id`/name/position/asset_id (fe-database/src/lib.rs
/// §node-log append; handlers/crud.rs) — unscoped, it was a side channel for
/// reading any petal's node metadata. Re-add only behind a node-scoped
/// subquery substitution (like `NODE`/`IOT_READING` below) if a real
/// consumer appears — see AGENTS.md §query-guard.
/// `ASSET` deliberately excluded (DEC-C21, 2026-10-09): asset rows carry no
/// owner/scope column, so allow-listing them exposed every uploaded asset's
/// `asset_id`/`content_hash` to any token; asset reads go through the scoped
/// `/api/v1/assets/*` + `/nodes/{id}/asset` routes (§assets).
const ALLOWED_TABLES: &[&str] = &[
    "NODE",
    "VERSE",
    "FRACTAL",
    "PETAL",
    "FIELD_DEF",
    "MODEL",
    "ROOM",
    "CRATE_REGISTRY",
    "CRATE_ENTRY",
    "IOT_READING",
];

/// Tables whose rows carry `petal_id` and are scope-filtered at the source.
/// `PETAL`/`ROOM`/`MODEL`/`CRATE_REGISTRY` added 2026-10-09 (DEC-C19 N1) —
/// each has its own `petal_id` column (fe-database/src/schema.rs), verified
/// per-table before adding. `CRATE_ENTRY` has no `petal_id` column and stays
/// unscoped (deliberately).
const PETAL_SCOPED_TABLES: &[&str] = &[
    "NODE",
    "IOT_READING",
    "PETAL",
    "ROOM",
    "MODEL",
    "CRATE_REGISTRY",
];

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

/// How long `/query/elevated` awaits the DB thread: the DB-side budget plus
/// slack for the diff + broadcast (AGENTS.md §elevated-query).
pub const ELEVATED_REPLY_TIMEOUT_SECS: u64 = STATEMENT_TIMEOUT_SECS + 2;

const UNSUPPORTED_FROM: &str =
    "unsupported FROM target (only table names or subqueries are allowed)";

/// Which egress surface a guarded query serves (dialect strictness).
/// Comments are banned in EVERY mode (R5, 2026-10-09) — `Query` used to
/// allow them, which let a comment split a record-constructor needle like
/// `TYPE::THING` into `TYPE/**/::/**/THING`, invisible to a whitespace-only
/// scan, and reach an unscoped `type::thing(...)` dereference via an
/// allow-listed-but-unscoped FROM target (e.g. `verse`). See
/// `reject_comments` and AGENTS.md §query-guard.
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

/// Tables `/api/v1/query/elevated` may read or write — scene content only
/// (see AGENTS.md §elevated-query). Deliberately excluded: `ROLE`,
/// `VERSE_MEMBER`, `VERSE` (`created_by`/`default_access` feed
/// `role_manager::resolve_role`) — privilege data changes only through the
/// RBAC handlers; `NODE_LOG` (append-only history); `ASSET`, `CRATE_*`
/// (content-addressed / signature-checked supply chain).
pub const ELEVATED_TABLES: &[&str] = &["NODE", "FRACTAL", "PETAL", "FIELD_DEF", "MODEL", "ROOM"];

/// Whole-word bans in elevated SQL: schema/DDL, session switching (`USE`
/// would re-point the DB thread's connection), access grants, changefeeds,
/// import mode, graph edges, blocking/live statements, a caller `TIMEOUT`
/// (could raise the budget; `PARALLEL`/`EXPLAIN` must follow it), and
/// transaction control (would break the DB thread's atomic wrapper).
const ELEVATED_BANNED_KEYWORDS: &[&str] = &[
    "DEFINE", "REMOVE", "ALTER", "REBUILD", "INFO", "USE", "ACCESS", "SHOW", "OPTION", "RELATE",
    "SLEEP", "KILL", "LIVE", "TIMEOUT", "PARALLEL", "EXPLAIN", "BEGIN", "COMMIT", "CANCEL",
];

/// Statement verbs that get the DB-side `TIMEOUT` appended (AGENTS.md §elevated-query).
const ELEVATED_DML_VERBS: &[&str] = &["SELECT", "CREATE", "UPDATE", "UPSERT", "DELETE", "INSERT"];

/// A validated elevated script, ready for the DB thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevatedPlan {
    /// Top-level statements; each DML one ends with ` TIMEOUT 5s`.
    pub statements: Vec<String>,
    /// Lowercase, deduplicated write targets (empty = read-only script).
    pub write_tables: Vec<String>,
}

/// Clauses that may follow a mutation target (UPDATE/UPSERT/CREATE/DELETE).
const MUTATION_BOUNDARY_KEYWORDS: &[&str] = &[
    "SET", "UNSET", "CONTENT", "MERGE", "PATCH", "REPLACE", "WHERE", "RETURN", "TIMEOUT",
    "PARALLEL", "EXPLAIN", "WITH", "VERSION",
];

const UNSUPPORTED_ELEVATED_TARGET: &str = "unsupported target in elevated query (only plain \
     whitelisted table names are allowed — filter rows with WHERE, bind values via vars)";

/// Elevated-query validation: whole-word keyword bans plus whitelisting of
/// EVERY read (FROM) and write (UPDATE/UPSERT/CREATE/DELETE/INSERT INTO)
/// target. Write targets must be plain table names — record ids, `$params`,
/// escaped identifiers, subqueries and expressions fail closed, since their
/// table cannot be read off the text. Defence in depth only: the endpoint is
/// Owner-only and operator-gated (AGENTS.md §elevated-query).
pub fn validate_elevated_sql(sql: &str, allowed_tables: &[&str]) -> Result<ElevatedPlan, String> {
    // ASCII-only uppercasing keeps the validated text byte-aligned with `sql`.
    let sql_upper = sql.to_ascii_uppercase();
    let sql_upper = sql_upper.as_str();
    for keyword in ELEVATED_BANNED_KEYWORDS {
        if contains_word(sql_upper, keyword) {
            return Err(format!("{keyword} is not allowed in elevated queries"));
        }
    }
    // Reads that never name a FROM target: comments (could split a needle),
    // record constructors / `.to_record()`, and `table:key` literals.
    reject_comments(sql_upper)?;
    reject_record_constructors(sql_upper, sql_upper)?;
    reject_foreign_record_literals(sql_upper, allowed_tables)?;
    let bytes = sql_upper.as_bytes();
    let mut tables = Vec::new();
    let mut writes = Vec::new();
    for pos in word_positions(sql_upper, "FROM") {
        tables.extend(elevated_targets(bytes, pos + 4, TargetKind::Read)?);
    }
    for verb in ["UPDATE", "UPSERT", "CREATE", "DELETE"] {
        for pos in word_positions(sql_upper, verb) {
            // `INSERT … ON DUPLICATE KEY UPDATE field = …` names no target.
            if verb == "UPDATE" && preceded_by_words(bytes, pos, &["DUPLICATE", "KEY"]) {
                continue;
            }
            writes.extend(elevated_targets(
                bytes,
                pos + verb.len(),
                TargetKind::Write,
            )?);
        }
    }
    for pos in word_positions(sql_upper, "INSERT") {
        let mut i = skip_ascii_ws(bytes, pos + 6);
        while let Some(word) = ["RELATION", "IGNORE"].iter().find(|w| word_at(bytes, i, w)) {
            i = skip_ascii_ws(bytes, i + word.len());
        }
        if !word_at(bytes, i, "INTO") {
            return Err("INSERT must name its target table with INTO".to_string());
        }
    }
    for pos in word_positions(sql_upper, "INTO") {
        writes.extend(elevated_targets(bytes, pos + 4, TargetKind::Insert)?);
    }
    for table in tables.iter().chain(&writes) {
        if !allowed_tables.contains(&table.as_str()) {
            return Err(format!(
                "table '{table}' is not allowed in elevated queries"
            ));
        }
    }
    let mut write_tables: Vec<String> = writes.iter().map(|t| t.to_ascii_lowercase()).collect();
    write_tables.sort();
    write_tables.dedup();
    let statements = split_elevated_statements(sql)
        .into_iter()
        .map(|statement| {
            let upper = statement.to_ascii_uppercase();
            if ELEVATED_DML_VERBS
                .iter()
                .any(|verb| word_at(upper.as_bytes(), 0, verb))
            {
                format!("{statement} TIMEOUT {STATEMENT_TIMEOUT_SECS}s")
            } else {
                statement
            }
        })
        .collect();
    Ok(ElevatedPlan {
        statements,
        write_tables,
    })
}

/// Split a script into trimmed, non-empty top-level statements: `;` splits
/// only outside strings (`'…'`/`"…"`, backslash escapes), escaped identifiers
/// (`⟨…⟩`/`` `…` ``) and `(…)`/`[…]`/`{…}` nesting (lexer modelled on
/// [`normalize_whitespace`]).
pub fn split_elevated_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let end = match c {
            '\'' | '"' | '`' | '⟨' => {
                let close = if c == '⟨' { '⟩' } else { c };
                let mut j = i + 1;
                while j < chars.len() && chars[j] != close {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                (j + 1).min(chars.len())
            }
            '(' | '[' | '{' => {
                depth += 1;
                i + 1
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                i + 1
            }
            ';' if depth == 0 => {
                statements.push(std::mem::take(&mut current));
                i += 1;
                continue;
            }
            _ => i + 1,
        };
        current.extend(&chars[i..end]);
        i = end;
    }
    statements.push(current);
    statements
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .filter(|statement| !statement.is_empty())
        .collect()
}

/// Which clause an elevated target list belongs to (decides what may follow it).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    /// FROM: SELECT clauses (or `RETURN` after `DELETE FROM`); `(SELECT …)` sources allowed.
    Read,
    /// UPDATE/UPSERT/CREATE/DELETE: mutation clauses; plain table names only.
    Write,
    /// INSERT INTO: the data follows directly (`{`, `[`, `(cols)`, `$data`).
    Insert,
}

/// The comma-separated target list starting at byte `i`; any element that is
/// not a plain identifier (or, for reads, a parenthesized SELECT whose own
/// FROM is scanned separately) fails closed.
fn elevated_targets(bytes: &[u8], mut i: usize, kind: TargetKind) -> Result<Vec<String>, String> {
    let unsupported = || UNSUPPORTED_ELEVATED_TARGET.to_string();
    i = skip_ascii_ws(bytes, i);
    if kind == TargetKind::Write {
        // `DELETE FROM x` / `UPDATE ONLY x` — the target list follows.
        while let Some(word) = ["FROM", "ONLY"].iter().find(|w| word_at(bytes, i, w)) {
            i = skip_ascii_ws(bytes, i + word.len());
        }
    }
    let mut tables = Vec::new();
    loop {
        i = skip_ascii_ws(bytes, i);
        if kind == TargetKind::Read && bytes.get(i) == Some(&b'(') {
            if !word_at(bytes, skip_ascii_ws(bytes, i + 1), "SELECT") {
                return Err(unsupported());
            }
            i = matching_paren(bytes, i).ok_or_else(unsupported)? + 1;
        } else {
            let start = i;
            while i < bytes.len() && is_ident(bytes[i]) {
                i += 1;
            }
            if i == start {
                return Err(unsupported());
            }
            tables.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        }
        let next = skip_ascii_ws(bytes, i);
        match bytes.get(next) {
            Some(b',') => i = next + 1,
            None | Some(b';' | b')') => return Ok(tables),
            Some(b'{' | b'[' | b'(' | b'$') if kind == TargetKind::Insert => return Ok(tables),
            Some(_) => {
                let boundaries: &[&str] = match kind {
                    TargetKind::Read => FROM_BOUNDARY_KEYWORDS,
                    TargetKind::Write => MUTATION_BOUNDARY_KEYWORDS,
                    TargetKind::Insert => &[],
                };
                let clean = boundaries.iter().any(|kw| word_at(bytes, next, kw))
                    || (kind == TargetKind::Read && word_at(bytes, next, "RETURN"));
                return if clean {
                    Ok(tables)
                } else {
                    Err(unsupported())
                };
            }
        }
    }
}

/// True if the whole words `words` (in order) are the last tokens before byte `pos`.
fn preceded_by_words(bytes: &[u8], mut pos: usize, words: &[&str]) -> bool {
    for word in words.iter().rev() {
        while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        if !(pos >= word.len()
            && bytes[..pos].ends_with(word.as_bytes())
            && (pos == word.len() || !is_ident(bytes[pos - word.len() - 1])))
        {
            return false;
        }
        pos -= word.len();
    }
    true
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

/// Reject comment tokens anywhere (string literals included) — banned in
/// EVERY [`GuardMode`] (R5, 2026-10-09): `Query` used to skip this call, and
/// a comment inside a function/operator sequence (e.g. `type/**/::`) is
/// inter-token trivia the SurrealQL lexer recurses through just like
/// whitespace, so it is not a cosmetic exemption.
fn reject_comments(sql: &str) -> Result<(), String> {
    for token in ["--", "#", "//", "/*"] {
        if sql.contains(token) {
            return Err(format!("comments ('{token}') are not allowed in queries"));
        }
    }
    Ok(())
}

/// Remove every comment span (`--`, `#`, `//` line comments; `/* … */` block
/// comments) OUTSIDE strings/escaped identifiers, DROPPING the bytes
/// entirely rather than collapsing them to whitespace like
/// `normalize_whitespace` does — independent, belt-and-suspenders defense
/// for [`reject_record_constructors`]'s needle scan (R5, 2026-10-09): a
/// comment can split a needle like `TYPE::THING` into
/// `TYPE/**/::/**/THING`, which a whitespace-only scan never catches. In the
/// normal call path `reject_comments` already runs first and unconditionally
/// (see [`prepare_scoped_sql`]), so by the time this runs there is nothing
/// left to strip — this only matters if that ordering ever regresses.
fn strip_comments(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' | '"' | '`' | '⟨' => {
                let close = if c == '⟨' { '⟩' } else { c };
                let start = i;
                let mut j = i + 1;
                while j < chars.len() && chars[j] != close {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(chars.len());
                out.extend(&chars[start..end]);
                i = end;
            }
            '#' => i = line_end(&chars, i),
            '-' if next == Some('-') => i = line_end(&chars, i),
            '/' if next == Some('/') => i = line_end(&chars, i),
            '/' if next == Some('*') => {
                let mut j = i + 2;
                while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                    j += 1;
                }
                i = (j + 2).min(chars.len());
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Reject every way to name a record/table WITHOUT the table's name appearing
/// as a plain word (escaped identifiers, record constructors incl. the
/// `.to_record()` method, `r"..."`) — shared by the scoped dialect and
/// elevated SQL (AGENTS.md §query-guard, §elevated-query).
fn reject_record_constructors(canonical: &str, upper: &str) -> Result<(), String> {
    if canonical.contains('`') || canonical.contains('⟨') || canonical.contains('⟩') {
        return Err("escaped identifiers (`...` / ⟨...⟩) are not allowed in queries".into());
    }
    // `strip_comments` first (R5): see its doc for why this is independent
    // belt-and-suspenders, not the primary defense.
    let scanned = strip_comments(upper);
    let compact: String = scanned.chars().filter(|c| !c.is_whitespace()).collect();
    for needle in [
        "TYPE::THING",
        "TYPE::RECORD",
        "TYPE::TABLE",
        // `type::field(s)` parse a string as an idiom at runtime.
        "TYPE::FIELD",
        "RECORD::",
        "<RECORD",
        // Method alias of `type::record` (`$s.to_record()`): a bound string
        // var becomes a record of ANY table (2026-10-10 review).
        "TO_RECORD",
    ] {
        if compact.contains(needle) {
            return Err(format!(
                "record/table constructors ({}) are not allowed in queries",
                needle.to_ascii_lowercase()
            ));
        }
    }
    let bytes = scanned.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'R'
            && matches!(bytes[i + 1], b'\'' | b'"')
            && (i == 0 || !is_ident(bytes[i - 1]))
        {
            return Err("record-id string literals (r\"...\") are not allowed in queries".into());
        }
    }
    Ok(())
}

/// Reject `table:key` record-id literals whose table is not in `readable`
/// (uppercased). Lexer-free: string contents are scanned too. SurrealQL
/// glues the `:` to both the table and the key (no whitespace), so spaced
/// object keys (`{k: v}`) and `::` paths pass; digit-led runs (times inside
/// datetime strings) can never name a real table — AGENTS.md §query-guard.
fn reject_foreign_record_literals(sql_upper: &str, readable: &[&str]) -> Result<(), String> {
    let bytes = sql_upper.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_ident(bytes[i]) {
            i += 1;
        }
        let glued_key = bytes.get(i) == Some(&b':')
            && bytes
                .get(i + 1)
                .is_some_and(|b| *b != b':' && !b.is_ascii_whitespace());
        let table = &sql_upper[start..i];
        if glued_key && !bytes[start].is_ascii_digit() && !readable.contains(&table) {
            return Err(format!(
                "record id on table '{table}' is not allowed in queries (bind string values \
                 via vars; write object keys as `key: value`)"
            ));
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
    // R5 (2026-10-09): unconditional in every mode — `Query` used to skip
    // this, which let a comment token survive into `reject_record_
    // constructors`'s needle scan and split `TYPE::THING` apart. `Query`'s
    // extra freedom is scoped subqueries (below), never comments.
    reject_comments(trimmed)?;
    let canonical = normalize_whitespace(trimmed);
    validate_select_sql(&canonical)?;
    let upper = canonical.to_ascii_uppercase();
    reject_record_constructors(&canonical, &upper)?;
    reject_foreign_record_literals(&upper, ALLOWED_TABLES)?;
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
/// comment so it cannot swallow the clause — defense in depth only: every
/// `GuardMode` now rejects comments before SQL reaches this point (R5,
/// 2026-10-09), so there should never be one left to swallow anything.
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

/// Concurrent channel-fallback egress queries allowed before a fresh caller
/// fails closed (DEC-C19 N3) — the single Windows DB thread's budget, not a
/// per-token rate: `/query`'s 10 rps is per-DID and share redemption's 10 rps
/// is per-token, but tokens/DIDs are unlimited to mint, so without a shared
/// cap unlimited concurrent callers can saturate the one DB thread with 5s
/// `TIMEOUT` statements indefinitely.
const EGRESS_FALLBACK_PERMITS: usize = 4;

/// Channel-fallback error when the API→DB command channel is closed.
const DB_BRIDGE_CLOSED: &str = "internal channel closed";
/// Channel-fallback error when the DB bridge dropped the reply unanswered.
const DB_BRIDGE_CANCELLED: &str = "request cancelled";

/// How long a channel-fallback request waits for a free permit before
/// failing closed with a clean, honest error — short on purpose: queueing
/// here would silently eat into `CLIENT_TIMEOUT_SECS`, which the caller
/// already budgets as pure DB-service time.
const EGRESS_FALLBACK_ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Global semaphore over the channel-fallback branch of
/// [`run_guarded_query_via_state`] ONLY — the direct `db_reader` path and
/// fe-ui's GUI queries never touch this and are never throttled by it.
static EGRESS_FALLBACK_SEMAPHORE: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(EGRESS_FALLBACK_PERMITS);

/// Acquire one channel-fallback egress permit, or a clean `"egress busy"`
/// error (DEC-C19 N3) when all [`EGRESS_FALLBACK_PERMITS`] are held past
/// [`EGRESS_FALLBACK_ACQUIRE_TIMEOUT`] — bounded and honest, never a silent
/// queue.
async fn acquire_egress_fallback_permit() -> Result<tokio::sync::SemaphorePermit<'static>, String> {
    match tokio::time::timeout(
        EGRESS_FALLBACK_ACQUIRE_TIMEOUT,
        EGRESS_FALLBACK_SEMAPHORE.acquire(),
    )
    .await
    {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(
            "egress busy (too many concurrent channel-fallback queries); retry shortly".to_string(),
        ),
    }
}

/// Execute a guarded query, preferring the direct `db_reader` and falling
/// back to the `DbCommand::RawQuery` gateway channel when no direct reader is
/// wired (F10: the Windows SurrealKV per-handle lock leaves `db_reader`
/// `None` on the deployment platform). The channel request is CORRELATED (M4
/// fix B2) and mirrors the direct path's TIMEOUT + row cap exactly. The
/// fallback branch is additionally bounded by [`EGRESS_FALLBACK_SEMAPHORE`]
/// (DEC-C19 N3); the direct `db_reader` return above is never throttled.
pub async fn run_guarded_query_via_state(
    state: &ApiState,
    guarded: &GuardedQuery,
    vars: &std::collections::HashMap<String, serde_json::Value>,
    row_cap: usize,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(ref db) = state.db_reader {
        return run_guarded_query(db, guarded, vars, row_cap).await;
    }
    let _egress_permit = acquire_egress_fallback_permit().await?;
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
        .map_err(|_| DB_BRIDGE_CLOSED.to_string())?;
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
        Ok(Err(_)) => return Err(DB_BRIDGE_CANCELLED.to_string()),
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

/// True when the channel fallback could not reach the DB thread at all
/// (command channel closed, or the reply dropped unanswered).
pub fn is_db_bridge_error(e: &str) -> bool {
    e == DB_BRIDGE_CLOSED || e == DB_BRIDGE_CANCELLED
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
            // 2026-10-10: method alias + literals of tables outside the whitelist.
            "SELECT $v.to_record().* AS leak FROM verse",
            "SELECT $v . to_record ( ) AS leak FROM verse",
            "SELECT role:victim.* AS leak FROM verse",
            "SELECT api_token:..1 AS leak FROM verse",
            "SELECT type::field('x') FROM verse",
        ] {
            assert!(
                prepare_scoped_sql(sql, &p(&["P1"]), GuardMode::Query).is_err(),
                "should reject: {sql}"
            );
        }
        // Not record ids: datetimes (digit-led), spaced object keys, `::` paths,
        // and literals of whitelisted unscoped tables (already FROM-readable).
        for sql in [
            "SELECT * FROM verse WHERE created_at > '2026-01-01T00:00:00Z'",
            "SELECT { k: 1 } AS o, string::lowercase(name) AS n FROM verse",
            "SELECT verse:abc.name AS n FROM verse",
        ] {
            assert!(
                prepare_scoped_sql(sql, &p(&["P1"]), GuardMode::Query).is_ok(),
                "should accept: {sql}"
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

    #[test]
    fn elevated_write_targets_must_be_plain_whitelisted_tables() {
        const TABLES: &[&str] = &["NODE", "PETAL"];
        for (sql, allowed) in [
            ("UPDATE NODE SET X = 1 WHERE NODE_ID = $ID", true),
            ("DELETE FROM NODE WHERE NODE_ID = $ID RETURN BEFORE", true),
            ("CREATE NODE CONTENT $DATA; SELECT * FROM PETAL;", true),
            ("INSERT INTO NODE $ROWS ON DUPLICATE KEY UPDATE X = 1", true),
            ("UPDATE NODE, PETAL SET X = 1", true),
            // Verbs the old FROM/INTO/UPDATE-only scan never inspected.
            ("DELETE ROLE", false),
            ("CREATE ROLE CONTENT {}", false),
            ("UPSERT ROLE SET R = 1", false),
            ("INSERT [{}]", false),
            // Later list elements, and targets whose table the text cannot show.
            ("UPDATE NODE, ROLE SET R = 1", false),
            ("UPDATE NODE AND ROLE SET R = 1", false),
            ("UPDATE ROLE:ABC SET R = 1", false),
            ("UPDATE $R SET R = 1", false),
            ("UPDATE `ROLE` SET R = 1", false),
            ("UPDATE (SELECT * FROM NODE) SET R = 1", false),
            ("UPDATE NODE->OWNS->ROLE SET R = 1", false),
            // Session switch / grants / edges.
            ("SELECT * FROM NODE; USE DB OTHER", false),
            ("ACCESS TOKEN GRANT FOR USER ROOT", false),
            ("RELATE NODE:A->EDGE->NODE:B", false),
            // Reads that name no FROM target (2026-10-10 review).
            ("RETURN ROLE:VICTIM.*", false),
            ("SELECT *, ROLE:VICTIM.* AS L FROM NODE", false),
            ("RETURN $V.TO_RECORD().*", false),
            ("RETURN TYPE::RECORD('ROLE', 'X').*", false),
            ("RETURN R'ROLE:X'.*", false),
            ("RETURN ⟨ROLE⟩:X.*", false),
            ("RETURN TYPE/**/::/**/RECORD('ROLE', 'X')", false),
            ("CREATE NODE CONTENT {ROLE:'OWNER'}", false),
            ("CREATE NODE CONTENT {ROLE: 'OWNER'}", true),
            // Only `ON DUPLICATE KEY UPDATE` skips the UPDATE-target check.
            ("RETURN $KEY UPDATE ROLE SET R = 1", false),
            (
                "UPDATE NODE SET PARENT = NODE:ABC WHERE ID = NODE:XYZ",
                true,
            ),
            // A caller budget or transaction control would escape the DB
            // thread's TIMEOUT / atomic wrapper.
            ("SELECT * FROM NODE TIMEOUT 1H", false),
            ("UPDATE NODE SET X = 1 PARALLEL", false),
            ("SELECT * FROM NODE EXPLAIN", false),
            ("BEGIN TRANSACTION; UPDATE NODE SET X = 1", false),
            ("UPDATE NODE SET X = 1; COMMIT TRANSACTION", false),
            ("UPDATE NODE SET X = 1; CANCEL TRANSACTION", false),
        ] {
            assert_eq!(
                validate_elevated_sql(sql, TABLES).is_ok(),
                allowed,
                "{sql}: {:?}",
                validate_elevated_sql(sql, TABLES)
            );
        }
    }

    #[test]
    fn elevated_plan_bounds_every_dml_and_names_its_write_tables() {
        const TABLES: &[&str] = &["NODE", "PETAL", "FRACTAL"];
        let plan = validate_elevated_sql(
            "LET $n = 'a;b'; update node set display_name = $n where node_id = $id; \
             SELECT * FROM petal;INSERT INTO Fractal [{ name: \"x;y\" }];  ; \
             UPDATE node SET tags = { a: 1; }; RETURN $n",
            TABLES,
        )
        .expect("valid script");
        assert_eq!(
            plan.statements,
            vec![
                "LET $n = 'a;b'".to_string(),
                "update node set display_name = $n where node_id = $id TIMEOUT 5s".to_string(),
                "SELECT * FROM petal TIMEOUT 5s".to_string(),
                "INSERT INTO Fractal [{ name: \"x;y\" }] TIMEOUT 5s".to_string(),
                "UPDATE node SET tags = { a: 1; } TIMEOUT 5s".to_string(),
                "RETURN $n".to_string(),
            ],
            "split only at top-level `;`, case preserved, TIMEOUT on DML only"
        );
        assert_eq!(plan.write_tables, vec!["fractal", "node"]);

        let read_only =
            validate_elevated_sql("SELECT * FROM node WHERE x = $x", TABLES).expect("read-only");
        assert!(read_only.write_tables.is_empty());
        assert_eq!(
            split_elevated_statements(r"SELECT 'a\';b' FROM node; RETURN 1"),
            vec![r"SELECT 'a\';b' FROM node", "RETURN 1"],
            "an escaped quote does not end the string"
        );
    }

    // ── DEC-C19 N1 — node_log removed, petal/room/model/crate_registry scoped ──

    #[test]
    fn node_log_is_no_longer_an_allowed_table() {
        // DEC-C19 N1: NODE_LOG has no documented /query/export/share consumer
        // and its payload carries another node's petal_id/name/position/
        // asset_id — it must be unreachable from every guarded mode.
        for mode in [GuardMode::Query, GuardMode::Egress, GuardMode::Export] {
            let err = prepare_scoped_sql("SELECT * FROM node_log", &p(&["P1"]), mode).unwrap_err();
            assert!(
                err.contains("'NODE_LOG'") && err.contains("not allowed"),
                "{mode:?}: {err}"
            );
        }
    }

    /// DEC-C19 N1: `petal`/`room`/`model`/`crate_registry` get the same
    /// FROM-substitution row filter as `node`/`iot_reading` — a scoped read
    /// of any of them sees only the caller's own petal(s), and mentioning
    /// one outside a FROM target fails closed exactly like `node` already
    /// does in `scoped_dialect_rejects_unscoped_record_access` (the scanner
    /// in `prepare_scoped_sql` is generic over `PETAL_SCOPED_TABLES`, so this
    /// is a list-membership change, not new logic).
    #[test]
    fn newly_petal_scoped_tables_are_rewritten_and_fail_closed_outside_from() {
        for table in ["petal", "room", "model", "crate_registry"] {
            let sql = format!("SELECT * FROM {table}");
            let out = prepare_scoped_sql(&sql, &p(&["P1"]), GuardMode::Query).unwrap();
            assert_eq!(
                out.sql,
                format!("SELECT * FROM (SELECT * FROM {table} WHERE petal_id = 'P1')")
            );

            // A mention outside the FROM target (lexer-free scan, strings
            // included) must reject — same fail-closed posture `node` gets.
            let leak = format!("SELECT * FROM verse WHERE name = '{table}'");
            assert!(
                prepare_scoped_sql(&leak, &p(&["P1"]), GuardMode::Query).is_err(),
                "should reject: {leak}"
            );
        }
    }

    // ── DEC-C19 N3 — bounded channel-fallback egress concurrency ──────────────

    #[tokio::test]
    async fn egress_fallback_permit_fails_closed_when_saturated_then_recovers() {
        // Exhaust every permit — a fresh caller must fail closed with a
        // clean "egress busy" error inside the short acquire timeout rather
        // than hang past it.
        let mut held = Vec::new();
        for _ in 0..EGRESS_FALLBACK_PERMITS {
            held.push(
                acquire_egress_fallback_permit()
                    .await
                    .expect("permit available"),
            );
        }
        let started = std::time::Instant::now();
        let err = acquire_egress_fallback_permit().await.unwrap_err();
        assert!(err.starts_with("egress busy"), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "must fail closed within the short acquire timeout, not hang"
        );

        // Releasing one permit frees capacity for the next caller.
        held.pop();
        assert!(acquire_egress_fallback_permit().await.is_ok());
    }

    // ── R5 — comments banned in every mode; needle scan comment-proof ──────────

    /// R5 (2026-10-09, independent grammar audit): `GuardMode::Query` used to
    /// skip `reject_comments`, so a comment could split the `TYPE::THING`
    /// needle apart (`type/**/::/**/thing(...)`) — invisible to the old
    /// whitespace-only scan — and reach an unscoped record dereference via
    /// `FROM verse` (allow-listed, never row-scoped). Comments are now
    /// rejected in every mode, before the needle scan ever runs.
    #[test]
    fn comments_are_banned_in_every_guard_mode() {
        let block_comment_attack =
            "SELECT *, type/**/::/**/thing('no' + 'de', 'n1').* AS leak FROM verse";
        let dash_comment_attack =
            "SELECT *, type::thing('no' + 'de', 'n1').* AS leak FROM verse -- x";
        for sql in [block_comment_attack, dash_comment_attack] {
            for mode in [GuardMode::Query, GuardMode::Egress, GuardMode::Export] {
                assert!(
                    prepare_scoped_sql(sql, &p(&["P1"]), mode).is_err(),
                    "{mode:?} should reject: {sql}"
                );
            }
        }
        // The pre-existing (comment-free) obfuscation — string concatenation
        // only — must stay rejected too: the needle scan still catches a
        // bare `type::thing(...)` with no comment involved.
        assert!(prepare_scoped_sql(
            "SELECT type::thing('no' + 'de', 'x').* FROM verse",
            &p(&["P1"]),
            GuardMode::Query
        )
        .is_err());
    }

    /// Belt-and-suspenders (R5): `reject_record_constructors`'s needle scan
    /// must catch the obfuscated needle even called directly on text that
    /// still carries comment bytes, independent of `reject_comments` having
    /// already run — the ordering in `prepare_scoped_sql` is not the only
    /// thing standing between this needle and a false negative.
    #[test]
    fn reject_record_constructors_is_comment_proof_independent_of_reject_comments() {
        let canonical =
            "select *, type/**/::/**/thing('no' + 'de', 'n1').* as leak from verse".to_string();
        let upper = canonical.to_ascii_uppercase();
        assert!(reject_record_constructors(&canonical, &upper).is_err());
    }

    #[test]
    fn strip_comments_drops_every_comment_form_outside_strings() {
        // Each comment form (and its trailing newline, for the line forms)
        // is dropped entirely; text before and after survives untouched.
        assert_eq!(strip_comments("A/**/B"), "AB");
        assert_eq!(strip_comments("A--x\nB"), "AB");
        assert_eq!(strip_comments("A#x\nB"), "AB");
        assert_eq!(strip_comments("A//x\nB"), "AB");
        // Comment-like bytes inside a string literal are preserved verbatim.
        assert_eq!(strip_comments("SELECT 'a/*b*/c'"), "SELECT 'a/*b*/c'");
    }
}
