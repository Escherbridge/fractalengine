//! DuckDB-to-DataFusion SQL dialect translator.
//!
//! DuckDB supports several SQL extensions that DataFusion does not understand.
//! This module provides a best-effort, string-based rewriting pass so that
//! common DuckDB idioms can be fed directly to DataFusion.
//!
//! No external dependencies -- all transformations use pure `std` string ops.

/// Translate DuckDB SQL extensions to DataFusion-compatible SQL.
///
/// Supported translations:
/// - `EXCLUDE (col1, col2)` -- stripped (full expansion requires schema context)
/// - `COLUMNS(*)` -- expanded to `*` (DataFusion native)
/// - `struct.field` -- `struct['field']` for known struct columns
/// - `//` line comments -- `--` comments
/// - `ILIKE` -- rewritten to `lower(col) LIKE lower(pattern)`
///
/// Translations are applied in sequence; the output of one feeds into the next.
pub fn translate(sql: &str) -> String {
    let sql = translate_comments(sql);
    let sql = translate_columns_star(&sql);
    let sql = translate_exclude(&sql);
    let sql = translate_ilike(&sql);
    translate_struct_access(&sql)
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Known struct column names from the entity schema. Only dot-access on these
/// names is rewritten to bracket syntax; plain `table.column` is left alone.
const STRUCT_COLUMNS: &[&str] = &["position", "rotation", "scale"];

/// Strip `EXCLUDE (col, ...)` clauses. Full column expansion requires schema
/// context that is not available here, so we simply remove the clause and
/// append a SQL comment noting the omission.
fn translate_exclude(sql: &str) -> String {
    let upper = sql.to_uppercase();
    if let Some(exc_start) = find_keyword(&upper, "EXCLUDE") {
        // Find the opening and closing parens after EXCLUDE.
        if let Some(paren_start) = sql[exc_start..].find('(') {
            let abs_paren = exc_start + paren_start;
            if let Some(paren_end) = sql[abs_paren..].find(')') {
                let abs_end = abs_paren + paren_end + 1; // include ')'
                let mut result = String::with_capacity(sql.len());
                result.push_str(sql[..exc_start].trim_end());
                result.push_str(&sql[abs_end..]);
                result.push_str(" /* column exclusion stripped -- expand at schema layer */");
                return result;
            }
        }
    }
    sql.to_string()
}

/// Replace `COLUMNS(*)` with plain `*`.
fn translate_columns_star(sql: &str) -> String {
    let upper = sql.to_uppercase();
    let needle = "COLUMNS";
    let mut result = String::with_capacity(sql.len());
    let mut cursor = 0;

    while let Some(pos) = find_keyword_at(&upper, needle, cursor) {
        let after = &sql[pos + needle.len()..];
        let trimmed = after.trim_start();
        let ws_len = after.len() - trimmed.len();

        if trimmed.starts_with("(*)") {
            result.push_str(&sql[cursor..pos]);
            result.push('*');
            cursor = pos + needle.len() + ws_len + 3; // skip "COLUMNS" + ws + "(*)"
        } else {
            result.push_str(&sql[cursor..pos + needle.len()]);
            cursor = pos + needle.len();
        }
    }
    result.push_str(&sql[cursor..]);
    result
}

/// Rewrite `col ILIKE pattern` to `lower(col) LIKE lower(pattern)`.
fn translate_ilike(sql: &str) -> String {
    let upper = sql.to_uppercase();
    let needle = "ILIKE";
    let mut result = String::with_capacity(sql.len());
    let mut cursor = 0;

    while let Some(pos) = find_keyword_at(&upper, needle, cursor) {
        // Walk backward from ILIKE to find the column identifier.
        let before = &sql[cursor..pos];
        let trimmed = before.trim_end();
        let col_start = trimmed
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let col = &trimmed[col_start..];
        let prefix = &trimmed[..col_start];

        // Walk forward from ILIKE to find the pattern value.
        let after = &sql[pos + needle.len()..];
        let after_trimmed = after.trim_start();
        let ws_after = after.len() - after_trimmed.len();
        let (pattern, pattern_len) = extract_value(after_trimmed);
        let consumed = pos + needle.len() + ws_after + pattern_len;

        result.push_str(&before[..cursor.min(before.len()).max(0)]);
        // We need to push the prefix within `before` (up to cursor offset).
        result.push_str(prefix);
        result.push_str(&format!("lower({col}) LIKE lower({pattern})"));

        cursor = consumed;
    }
    result.push_str(&sql[cursor..]);
    result
}

/// Rewrite `struct.field` to `struct['field']` for known struct column names.
///
/// Only rewrites when the left-hand identifier is in [`STRUCT_COLUMNS`] to
/// avoid mangling ordinary `table.column` references.
fn translate_struct_access(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut result = String::with_capacity(sql.len());
    let mut i = 0;

    while i < chars.len() {
        // Try to read an identifier.
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();

            // Check for dot + field access on struct columns.
            if i < chars.len()
                && chars[i] == '.'
                && STRUCT_COLUMNS.contains(&ident.to_lowercase().as_str())
            {
                let dot = i;
                i += 1; // skip dot
                let field_start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if i > field_start {
                    let field: String = chars[field_start..i].iter().collect();
                    result.push_str(&ident);
                    result.push_str(&format!("['{field}']"));
                } else {
                    // Dot not followed by an identifier -- leave as-is.
                    result.push_str(&ident);
                    result.push(chars[dot]);
                }
            } else {
                result.push_str(&ident);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Replace `//` line comments with `--` line comments.
fn translate_comments(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut in_single_quote = false;

    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\'' && !in_single_quote {
            in_single_quote = true;
            result.push(chars[i]);
            i += 1;
        } else if chars[i] == '\'' && in_single_quote {
            in_single_quote = false;
            result.push(chars[i]);
            i += 1;
        } else if !in_single_quote && i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/'
        {
            result.push_str("--");
            i += 2;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ── Utility functions ───────────────────────────────────────────────────────

/// Find a keyword in `haystack` that is bounded by non-alphanumeric chars
/// (i.e. a whole-word match), starting from position 0.
fn find_keyword(haystack: &str, keyword: &str) -> Option<usize> {
    find_keyword_at(haystack, keyword, 0)
}

/// Find a keyword at or after `from`, with word-boundary checks.
fn find_keyword_at(haystack: &str, keyword: &str, from: usize) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let kw_bytes = keyword.as_bytes();
    let kw_len = kw_bytes.len();

    let mut pos = from;
    while pos + kw_len <= bytes.len() {
        if let Some(idx) = haystack[pos..].find(keyword) {
            let abs = pos + idx;
            let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
            let after_ok =
                abs + kw_len >= bytes.len() || !bytes[abs + kw_len].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(abs);
            }
            pos = abs + 1;
        } else {
            break;
        }
    }
    None
}

/// Extract a SQL value token (quoted string or identifier/param) from `s`.
/// Returns `(value, bytes_consumed)`.
fn extract_value(s: &str) -> (&str, usize) {
    if s.starts_with('\'') {
        // Quoted string -- find closing quote (handling escapes).
        let mut i = 1;
        let bytes = s.as_bytes();
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                i += 2;
            } else if bytes[i] == b'\'' {
                return (&s[..i + 1], i + 1);
            } else {
                i += 1;
            }
        }
        (s, s.len())
    } else {
        // Identifier, bind param ($1, :name, ?)
        let end = s
            .find(|c: char| c.is_whitespace() || c == ')' || c == ',')
            .unwrap_or(s.len());
        (&s[..end], end)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_exclude_stripped() {
        let input = "SELECT * EXCLUDE (petal_id, updated_at_ms) FROM nodes";
        let out = translate(input);
        assert!(
            !out.contains("EXCLUDE"),
            "EXCLUDE clause should be removed, got: {out}"
        );
        assert!(
            out.contains("column exclusion stripped"),
            "should leave a comment"
        );
        assert!(out.contains("SELECT *"));
        assert!(out.contains("FROM nodes"));
    }

    #[test]
    fn translate_exclude_case_insensitive() {
        let input = "SELECT * exclude (foo) FROM t";
        let out = translate(input);
        assert!(
            !out.contains("exclude (") && !out.contains("EXCLUDE ("),
            "EXCLUDE clause should be removed, got: {out}"
        );
    }

    #[test]
    fn translate_struct_access() {
        let input = "SELECT position.x, rotation.y, scale.z FROM nodes";
        let out = translate(input);
        assert!(out.contains("position['x']"), "got: {out}");
        assert!(out.contains("rotation['y']"), "got: {out}");
        assert!(out.contains("scale['z']"), "got: {out}");
    }

    #[test]
    fn translate_struct_access_ignores_table_column() {
        let input = "SELECT nodes.node_id FROM nodes";
        let out = translate(input);
        assert!(
            out.contains("nodes.node_id"),
            "table.column should NOT be rewritten, got: {out}"
        );
    }

    #[test]
    fn translate_comments() {
        let input = "SELECT * FROM nodes // all nodes\n// second line";
        let out = translate(input);
        assert!(!out.contains("//"), "got: {out}");
        assert!(out.contains("-- all nodes"), "got: {out}");
        assert!(out.contains("-- second line"), "got: {out}");
    }

    #[test]
    fn translate_ilike() {
        let input = "SELECT * FROM nodes WHERE name ILIKE '%foo%'";
        let out = translate(input);
        assert!(!out.to_uppercase().contains("ILIKE"), "got: {out}");
        assert!(
            out.contains("lower(name) LIKE lower('%foo%')"),
            "got: {out}"
        );
    }

    #[test]
    fn translate_ilike_with_bind_param() {
        let input = "SELECT * FROM nodes WHERE name ILIKE $1";
        let out = translate(input);
        assert!(out.contains("lower(name) LIKE lower($1)"), "got: {out}");
    }

    #[test]
    fn translate_columns_star() {
        let input = "SELECT COLUMNS(*) FROM nodes";
        let out = translate(input);
        assert!(out.contains("SELECT * FROM"), "got: {out}");
    }

    #[test]
    fn translate_passthrough() {
        let input = "SELECT node_id, petal_id FROM nodes WHERE updated_at_ms > 100";
        let out = translate(input);
        assert_eq!(out, input, "plain SQL should pass through unchanged");
    }

    #[test]
    fn translate_combined() {
        let input = "SELECT * EXCLUDE (petal_id) FROM nodes WHERE name ILIKE '%test%' // filter";
        let out = translate(input);
        assert!(!out.to_uppercase().contains(" ILIKE "), "got: {out}");
        assert!(!out.contains("//"), "got: {out}");
        assert!(
            !out.contains("EXCLUDE (") && !out.contains("exclude ("),
            "got: {out}"
        );
    }
}
