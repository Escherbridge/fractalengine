//! `DbCommand::RawQuery` reply (extracted from the dispatch loop so tests drive
//! production code — fe-database/src/AGENTS.md §handlers).

use std::collections::HashMap;

use fe_runtime::messages::DbResult;

use crate::repo::Db;

/// Statement keywords the DB thread refuses even inside a leading `SELECT`.
const BLOCKED_KEYWORDS: [&str; 20] = [
    "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT", "LET", "RETURN", "INFO",
    "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL", "IF", "BEGIN", "COMMIT", "CANCEL",
];

/// True when `keyword` appears in `sql_upper` as a whole word.
fn contains_keyword(sql_upper: &str, keyword: &str) -> bool {
    let bytes = sql_upper.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0;
    while let Some(pos) = sql_upper[start..].find(keyword) {
        let abs = start + pos;
        let after_pos = abs + keyword.len();
        let before = abs == 0 || !is_word(bytes[abs - 1]);
        let after = after_pos >= bytes.len() || !is_word(bytes[after_pos]);
        if before && after {
            return true;
        }
        start = after_pos;
    }
    false
}

/// The DB thread's reply to `RawQuery`: only a single `SELECT` is run. A
/// correlated (API) caller gets `QueryResult`/`QueryFailed` echoing its id; a
/// GUI query (`None`) gets `QueryResult { None }` or the wildcard `Error`.
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
pub async fn raw_query_reply(
    db: &Db,
    sql: &str,
    vars: &HashMap<String, serde_json::Value>,
    correlation_id: Option<String>,
) -> DbResult {
    let query_failed = |error: String| match &correlation_id {
        Some(id) => DbResult::QueryFailed {
            error,
            correlation_id: id.clone(),
        },
        None => DbResult::Error(error),
    };
    let sql_trimmed = sql.trim();
    let sql_upper = sql_trimmed.to_uppercase();
    let blocked = sql_trimmed.contains(';')
        || !sql_upper.starts_with("SELECT")
        || BLOCKED_KEYWORDS
            .iter()
            .any(|keyword| contains_keyword(&sql_upper, keyword));
    if blocked {
        return query_failed("only single SELECT statements are allowed".to_string());
    }
    let mut query_builder = db.query(sql);
    for (key, value) in vars {
        query_builder = query_builder.bind((key.clone(), value.clone()));
    }
    // `.check()`: a failed statement (incl. a DB-side `TIMEOUT`) must never
    // read as an empty-but-successful result.
    match query_builder.await.and_then(|r| r.check()) {
        Ok(mut response) => {
            let mut data = Vec::new();
            for idx in 0..response.num_statements() {
                match response.take::<Vec<serde_json::Value>>(idx) {
                    Ok(rows) => data.extend(rows),
                    Err(_) => break,
                }
            }
            DbResult::QueryResult {
                data,
                correlation_id,
            }
        }
        Err(e) => query_failed(format!("query failed: {e}")),
    }
}
