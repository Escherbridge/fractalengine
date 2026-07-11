//! Extension query API — host-provided SELECT-only access for extensions.
//!
//! The engine binary implements [`ExtensionQueryApi`]; the plugin host routes
//! Rhai/WASM `query_select` calls through the injected trait object. The
//! [`is_select_only`] guard mirrors `DbCommand::RawQuery` byte-for-byte so the
//! same keyword-block policy is enforced without a `fe-database` dependency.
//! See `fe-sdk/src/AGENTS.md` §query.

use std::collections::HashMap;

/// Capability granting SELECT-only query access.
pub const CAP_QUERY_SELECT: &str = "query.select";

/// Max byte length accepted for a query string at the host boundary.
pub const MAX_QUERY_LEN: usize = 8192;
/// Max rows returned from a single `query_select` — caps runaway result sets.
pub const MAX_RESULT_ROWS: usize = 10_000;

/// SurrealQL keywords blocked in extension queries (write / control-flow / live).
///
/// Mirrors the block-list in `fe-database`'s `DbCommand::RawQuery` handler.
pub const BLOCKED_KEYWORDS: &[&str] = &[
    "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT", "LET", "RETURN", "INFO",
    "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL", "IF", "BEGIN", "COMMIT", "CANCEL",
];

/// Host-provided SELECT-only query access for extensions.
///
/// Object-safe so `fe-plugin` can hold an `Arc<dyn ExtensionQueryApi>`.
pub trait ExtensionQueryApi: Send + Sync {
    /// Execute a single SELECT statement with bound params, returning JSON rows.
    ///
    /// Implementations may assume the host already enforced [`is_select_only`],
    /// but should still bind `params` rather than string-interpolate them.
    fn query_select(
        &self,
        sql: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, QueryError>;
}

/// Whether `sql` is a single, safe SELECT statement.
///
/// Byte-for-byte the same guard idiom as `DbCommand::RawQuery`: rejects any
/// `;`, anything not starting with `SELECT`, and any blocked keyword appearing
/// as a whole word. Returns `true` only when the statement is allowed.
pub fn is_select_only(sql: &str) -> bool {
    let sql_trimmed = sql.trim();
    let sql_upper = sql_trimmed.to_uppercase();
    let blocked = sql_trimmed.contains(';')
        || !sql_upper.starts_with("SELECT")
        || BLOCKED_KEYWORDS.iter().any(|kw| contains_keyword(&sql_upper, kw));
    !blocked
}

/// Whole-word (identifier-boundary) keyword search over an upper-cased query.
fn contains_keyword(sql_upper: &str, kw: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = sql_upper[start..].find(kw) {
        let abs = start + pos;
        let before = abs == 0
            || (!sql_upper.as_bytes()[abs - 1].is_ascii_alphanumeric()
                && sql_upper.as_bytes()[abs - 1] != b'_');
        let after_pos = abs + kw.len();
        let after = after_pos >= sql_upper.len()
            || (!sql_upper.as_bytes()[after_pos].is_ascii_alphanumeric()
                && sql_upper.as_bytes()[after_pos] != b'_');
        if before && after {
            return true;
        }
        start = abs + kw.len();
    }
    false
}

/// Validate a query string's length at the host boundary before running it.
pub fn validate_query_len(sql: &str) -> Result<(), QueryError> {
    if sql.trim().is_empty() {
        return Err(QueryError::InvalidInput("query must not be empty".into()));
    }
    if sql.len() > MAX_QUERY_LEN {
        return Err(QueryError::InvalidInput(format!(
            "query exceeds {MAX_QUERY_LEN} bytes"
        )));
    }
    Ok(())
}

/// Errors surfaced by the query API. Always propagated — never swallowed.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryError {
    /// The extension lacks the `query.select` capability.
    CapabilityDenied(String),
    /// An argument failed host-boundary validation.
    InvalidInput(String),
    /// The statement was not a single safe SELECT.
    NotSelectOnly,
    /// No implementation was injected by the host binary.
    NotAvailable,
    /// The underlying query engine failed.
    Backend(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::CapabilityDenied(c) => write!(f, "capability '{c}' not granted"),
            QueryError::InvalidInput(m) => write!(f, "invalid input: {m}"),
            QueryError::NotSelectOnly => {
                write!(f, "only single SELECT statements are allowed")
            }
            QueryError::NotAvailable => write!(f, "query API not available"),
            QueryError::Backend(m) => write!(f, "query backend error: {m}"),
        }
    }
}

impl std::error::Error for QueryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_plain_select() {
        assert!(is_select_only("SELECT * FROM node"));
        assert!(is_select_only("  select name from node where petal = $p  "));
    }

    #[test]
    fn rejects_semicolons_and_writes() {
        assert!(!is_select_only("SELECT * FROM node; DELETE node"));
        assert!(!is_select_only("DELETE node"));
        assert!(!is_select_only("UPDATE node SET x = 1"));
        assert!(!is_select_only("SELECT * FROM node WHERE 1=1 RETURN 1"));
        assert!(!is_select_only("CREATE node"));
    }

    #[test]
    fn keyword_boundary_does_not_flag_substrings() {
        // "created_at" contains "CREATE" but not as a whole word.
        assert!(is_select_only("SELECT created_at FROM node"));
        // "information" contains "INFO" as a substring only.
        assert!(is_select_only("SELECT information FROM node"));
    }

    #[test]
    fn validate_query_len_bounds() {
        assert!(validate_query_len("").is_err());
        assert!(validate_query_len("SELECT 1").is_ok());
        assert!(validate_query_len(&"x".repeat(MAX_QUERY_LEN + 1)).is_err());
    }

    #[test]
    fn query_error_display() {
        assert_eq!(
            QueryError::NotSelectOnly.to_string(),
            "only single SELECT statements are allowed"
        );
    }
}
