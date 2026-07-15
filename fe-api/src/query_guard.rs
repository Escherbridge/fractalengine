//! Shared guard pipeline for /query + export/share egress — see `fe-api/AGENTS.md` §query-guard.
//!
//! One entry point (`guard_and_prepare_query`) so export/share handlers cannot
//! bypass any guard `execute_query` enforces. Logic moved verbatim from
//! `rest.rs::execute_query` (Task 2.1); error strings preserved.

use std::sync::Arc;

use crate::server::ApiState;

/// SQL that has passed every static guard, with the scope filter injected.
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

    let sql_upper = sql_trimmed.to_uppercase();
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
    for table_name in from_clause_tables(&sql_upper)? {
        if !ALLOWED_TABLES.contains(&table_name.as_str()) {
            return Err(format!(
                "queries against table '{table_name}' are not allowed"
            ));
        }
    }

    Ok(())
}

/// True if `word` appears as a whole word (not inside a longer identifier).
pub fn contains_word(sql_upper: &str, word: &str) -> bool {
    let bytes = sql_upper.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0;
    while let Some(pos) = sql_upper[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0 || !is_ident(bytes[abs_pos - 1]);
        let after_pos = abs_pos + word.len();
        let after_ok = after_pos >= bytes.len() || !is_ident(bytes[after_pos]);
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + word.len();
    }
    false
}

/// Extract the (uppercased) table name following the first FROM, if any.
pub fn from_table(sql_upper: &str) -> Option<String> {
    let from_pos = sql_upper.find("FROM")?;
    let after_from = sql_upper[from_pos + 4..].trim_start();
    let table_name: String = after_from
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    Some(table_name)
}

/// Every table identifier targeted by any FROM clause; errors on FROM
/// targets that cannot be whitelist-checked (variables, literals).
pub fn from_clause_tables(sql_upper: &str) -> Result<Vec<String>, String> {
    let bytes = sql_upper.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut tables = Vec::new();
    let mut start = 0;
    while let Some(pos) = sql_upper[start..].find("FROM") {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0 || !is_ident(bytes[abs_pos - 1]);
        let after_kw = abs_pos + 4;
        let after_ok = after_kw >= bytes.len() || !is_ident(bytes[after_kw]);
        start = after_kw;
        if !(before_ok && after_ok) {
            continue;
        }
        let mut rest = sql_upper[after_kw..].trim_start();
        // A parenthesized subquery's own FROM is caught by this same scan.
        if rest.starts_with('(') {
            continue;
        }
        loop {
            let table: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if table.is_empty() {
                return Err(
                    "unsupported FROM target (only table names or subqueries are allowed)"
                        .to_string(),
                );
            }
            rest = rest[table.len()..].trim_start();
            tables.push(table);
            match rest.strip_prefix(',') {
                Some(next) => rest = next.trim_start(),
                None => break,
            }
        }
    }
    Ok(tables)
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
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
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
                return Err(format!("table '{table}' is not allowed in elevated queries"));
            }
        }
    }
    Ok(())
}

/// Build a scope filter clause from the token's scope string.
/// Returns a WHERE clause fragment like `petal_id = 'p1'` or empty for broad access.
pub fn build_scope_filter(scope: &str) -> String {
    let Ok(parts) = fe_database::parse_scope(scope) else {
        return String::new();
    };
    if let Some(ref petal_id) = parts.petal_id {
        format!("petal_id = '{}'", petal_id.replace('\'', ""))
    } else {
        // Verse or fractal scope — broader access, pass through without filter.
        String::new()
    }
}

/// Inject scope filter into SQL that queries a petal_id-carrying table
/// (`node`, `iot_reading`). Simple heuristic: if the query touches such a
/// FROM target and we have a scope filter, append/inject a WHERE clause.
pub fn inject_scope_filter(sql: &str, scope_filter: &str) -> String {
    if scope_filter.is_empty() {
        return sql.to_string();
    }
    let sql_upper = sql.to_uppercase();
    if !sql_upper.contains("FROM NODE") && !sql_upper.contains("FROM IOT_READING") {
        return sql.to_string();
    }
    // If there's already a WHERE, add AND; otherwise add WHERE
    if sql_upper.contains("WHERE") {
        format!("{sql} AND {scope_filter}")
    } else {
        format!("{sql} WHERE {scope_filter}")
    }
}

/// Full guard chain: rate limit → static validation → scope-filter injection.
pub async fn guard_and_prepare_query(
    state: &ApiState,
    rate_key: &str,
    rate_max_per_sec: u32,
    rate_label: &str,
    scope: &str,
    sql: &str,
) -> Result<GuardedQuery, String> {
    check_rate_limit(state, rate_key, rate_max_per_sec, rate_label).await?;
    validate_select_sql(sql)?;
    let scope_filter = build_scope_filter(scope);
    Ok(GuardedQuery {
        sql: inject_scope_filter(sql.trim(), &scope_filter),
    })
}

/// Execute a guarded query with the pre-existing 5s statement timeout and a row cap.
pub async fn run_guarded_query(
    db: &Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>,
    guarded: &GuardedQuery,
    vars: &std::collections::HashMap<String, serde_json::Value>,
    row_cap: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let mut query_builder = db.query(&guarded.sql);
    for (key, value) in vars {
        query_builder = query_builder.bind((key.clone(), value.clone()));
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        query_builder.await
    })
    .await;

    match result {
        Ok(Ok(mut response)) => {
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
        Err(_) => Err("query timed out (5s)".to_string()),
    }
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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn iot_reading_table_whitelisted() {
        assert!(
            validate_select_sql("SELECT * FROM iot_reading WHERE metric = 'temperature_c'").is_ok()
        );
    }

    #[test]
    fn inject_covers_iot_reading_table() {
        assert_eq!(
            inject_scope_filter("SELECT * FROM iot_reading", "petal_id = 'p1'"),
            "SELECT * FROM iot_reading WHERE petal_id = 'p1'"
        );
        assert_eq!(
            inject_scope_filter(
                "SELECT * FROM iot_reading WHERE metric = 'co2_ppm'",
                "petal_id = 'p1'"
            ),
            "SELECT * FROM iot_reading WHERE metric = 'co2_ppm' AND petal_id = 'p1'"
        );
    }

    #[test]
    fn scope_filter_petal_level_only() {
        assert_eq!(
            build_scope_filter("VERSE#v1-FRACTAL#f1-PETAL#p1"),
            "petal_id = 'p1'"
        );
        assert_eq!(build_scope_filter("VERSE#v1"), "");
    }

    #[test]
    fn inject_adds_where_or_and() {
        assert_eq!(
            inject_scope_filter("SELECT * FROM node", "petal_id = 'p1'"),
            "SELECT * FROM node WHERE petal_id = 'p1'"
        );
        assert_eq!(
            inject_scope_filter("SELECT * FROM node WHERE x = 1", "petal_id = 'p1'"),
            "SELECT * FROM node WHERE x = 1 AND petal_id = 'p1'"
        );
        assert_eq!(
            inject_scope_filter("SELECT * FROM verse", "petal_id = 'p1'"),
            "SELECT * FROM verse"
        );
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
    fn from_table_extracts_first_target() {
        assert_eq!(
            from_table("SELECT * FROM NODE WHERE X = 1").as_deref(),
            Some("NODE")
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
        assert!(err.contains("session_cache") || err.contains("SESSION_CACHE"), "{err}");
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

    #[test]
    fn non_identifier_from_targets_are_rejected() {
        assert!(validate_select_sql("SELECT * FROM $tbl").is_err());
        assert!(validate_select_sql("SELECT * FROM type::table($t)").is_err());
    }

    #[test]
    fn rbac_tables_not_readable_via_egress() {
        assert!(validate_select_sql("SELECT * FROM role").is_err());
        assert!(validate_select_sql("SELECT * FROM verse_member").is_err());
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
