//! Shared guard pipeline for /query + export/share egress — see `fe-api/AGENTS.md` §query-guard.
//!
//! One entry point per surface (`guard_and_prepare_query[_with_mode]`,
//! `prepare_scoped_sql`) so export/share handlers cannot bypass any guard
//! `execute_query` enforces. Row scoping is FROM-substitution (M4 fix B1).

use std::sync::Arc;

use crate::server::ApiState;
pub use fe_query::sql_guard::*;

/// SQL that has passed every static guard, with the scope filter applied.
pub struct GuardedQuery {
    pub sql: String,
}

/// Client-side backstop; deliberately longer than the DB-side TIMEOUT so the
/// DB aborts (and frees its thread) first.
const CLIENT_TIMEOUT_SECS: u64 = STATEMENT_TIMEOUT_SECS + 1;

/// How long `/query/elevated` awaits the DB thread: the DB-side budget plus
/// slack for the diff + broadcast (AGENTS.md §elevated-query).
pub const ELEVATED_REPLY_TIMEOUT_SECS: u64 = STATEMENT_TIMEOUT_SECS + 2;

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

/// Execute a guarded query with a DB-side `TIMEOUT 5s`, a client backstop, and a row cap.
pub async fn run_guarded_query(
    db: &Arc<crate::db_reader::ReadDb>,
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

/// True when the channel fallback could not reach the DB thread at all
/// (command channel closed, or the reply dropped unanswered).
pub fn is_db_bridge_error(e: &str) -> bool {
    e == DB_BRIDGE_CLOSED || e == DB_BRIDGE_CANCELLED
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
}
