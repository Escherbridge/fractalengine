//! Distributed timeseries query fan-out (M2/F7 — A15/A16/A17) — see
//! `fe-api/AGENTS.md` §distributed-query.
//!
//! The one guarded bridge every distributed surface shares: `/api/v1/query`
//! (the structured `distributed` mode), the analytics endpoint's merged
//! `iot_reading` table, and the MCP `query_timeseries` tool. Authorization
//! happens HERE (Viewer+ role, petal scope resolution + token containment,
//! per-DID rate limit), the verse is derived from the resolved petal scope
//! (never from the request body), and the fan-out rides the API→sync seam
//! (`ApiState.distributed_tx` — the same channel shape the replication
//! bridge uses, bridged into `SyncCommand::SubmitComputeTask` by the binary).
//!
//! The spec is structured (`fe_runtime::distributed_query::TsQueryKind`),
//! never SQL: peers render their own partial SQL from it, so a request can
//! only ever read one petal's readings through one of the sanctioned shapes.

use fe_identity::api_token::ApiClaims;
use fe_runtime::distributed_query::{DistributedQueryCall, DistributedQueryOutcome, TsQueryKind};

use crate::auth::{require_role, require_scope};
use crate::limits;
use crate::query_guard;
use crate::server::ApiState;
use crate::types::is_valid_ulid;

/// Answer deadline for API-surfaced fan-outs. Bounded by the transport's own
/// `MAX_QUERY_TIMEOUT_MS`; generous over the sync-side default so a slow
/// fleet still answers within the HTTP request budget.
pub const DISTRIBUTED_QUERY_TIMEOUT_MS: u64 = 8_000;

/// Per-host partial row cap the API surfaces request. Zero selects the
/// transport default (`PARTIAL_ROW_CAP`) — the executing host clamps
/// anything it receives anyway, so the API layer never widens a cap.
pub const DISTRIBUTED_QUERY_ROW_CAP: usize = 0;

/// Guarded fan-out failures, mapped by each surface to its own error shape
/// (200+`ok:false` on `/query`, real statuses on the analytics surface,
/// `tool_error` on MCP) — the `IotIngestError` precedent.
#[derive(Debug)]
pub enum TimeseriesQueryError {
    Forbidden(&'static str),
    BadRequest(String),
    NotFound(&'static str),
    /// Rate-limited or the transport is not configured (dynamic messages).
    Unavailable(String),
    /// The sync-thread seam is gone or never answered.
    BadGateway(&'static str),
    /// The deadline passed before a merged outcome arrived.
    Timeout,
}

impl TimeseriesQueryError {
    /// The human-readable message every surface reports.
    pub fn message(&self) -> &str {
        match self {
            Self::Forbidden(m) => m,
            Self::BadRequest(m) => m,
            Self::NotFound(m) => m,
            Self::Unavailable(m) => m,
            Self::BadGateway(m) => m,
            Self::Timeout => "distributed query timed out",
        }
    }
}

/// Run one guarded distributed timeseries fan-out (see the module docs).
///
/// On success the outcome carries the merged rows plus the A16 honesty
/// metadata; `outcome.error` is `Some` when the transport itself refused
/// (no open fabric, queue full, …) — callers surface that as their error.
pub async fn run_distributed_timeseries_query(
    state: &ApiState,
    claims: &ApiClaims,
    spec: TsQueryKind,
) -> Result<DistributedQueryOutcome, TimeseriesQueryError> {
    // Reads need Viewer+, like every other read surface.
    if require_role(claims, "viewer").is_err() {
        return Err(TimeseriesQueryError::Forbidden("insufficient permissions"));
    }
    let petal_id = spec.petal_id().to_string();
    if !is_valid_ulid(&petal_id) {
        return Err(TimeseriesQueryError::BadRequest(
            "distributed query requires a valid petal_id in its spec".into(),
        ));
    }
    // Resolve the petal's hierarchical scope and require token containment —
    // the same deny-by-default shape the ingest path applies.
    let Some(scope) = crate::rest::resolve_petal_scope(state, &petal_id).await else {
        return Err(TimeseriesQueryError::NotFound("unknown petal"));
    };
    if require_scope(claims, &scope).is_err() {
        tracing::warn!(
            petal_id,
            sub = %claims.sub,
            "distributed query denied: token scope does not cover petal"
        );
        return Err(TimeseriesQueryError::Forbidden("insufficient scope"));
    }
    // The verse comes from the resolved scope, never from the request body.
    let Some(verse_id) = fe_database::parse_scope(&scope)
        .ok()
        .map(|parts| parts.verse_id)
    else {
        return Err(TimeseriesQueryError::NotFound("petal scope is malformed"));
    };
    // One rate-limit bucket per DID for every distributed fan-out — each one
    // spawns a bounded collector on the sync thread (MAX_CONCURRENT_QUERIES).
    let rate_key = format!("distributed:{}", claims.sub);
    if let Err(e) = query_guard::check_rate_limit(
        state,
        &rate_key,
        limits::QUERY_RATE_PER_SEC,
        "10 distributed queries/sec",
    )
    .await
    {
        return Err(TimeseriesQueryError::Unavailable(e));
    }
    let Some(distributed_tx) = state.distributed_tx.as_ref() else {
        return Err(TimeseriesQueryError::Unavailable(
            "distributed query transport not configured (no sync-thread seam)".into(),
        ));
    };
    let (reply_tx, reply_rx) = crossbeam::channel::bounded::<DistributedQueryOutcome>(1);
    let call = DistributedQueryCall {
        request: fe_runtime::distributed_query::DistributedQueryRequest {
            request_id: ulid::Ulid::new().to_string(),
            verse_id,
            spec,
            timeout_ms: DISTRIBUTED_QUERY_TIMEOUT_MS,
            row_cap: DISTRIBUTED_QUERY_ROW_CAP,
        },
        reply: reply_tx,
    };
    if let Err(e) = distributed_tx.send(call) {
        tracing::warn!("distributed query seam send failed (sync thread gone): {e:?}");
        return Err(TimeseriesQueryError::BadGateway(
            "distributed query transport unavailable",
        ));
    }
    // The reply rides a crossbeam receiver — await it off the async workers
    // (spawn_blocking), bounded by the request deadline plus slack for the
    // merge + reply hop.
    let wait = std::time::Duration::from_millis(DISTRIBUTED_QUERY_TIMEOUT_MS + 2_000);
    match tokio::task::spawn_blocking(move || reply_rx.recv_timeout(wait)).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) => Err(TimeseriesQueryError::BadGateway(
            "distributed query transport did not answer",
        )),
        Err(e) => {
            tracing::error!("distributed query reply task failed: {e:?}");
            Err(TimeseriesQueryError::BadGateway(
                "distributed query reply task failed",
            ))
        }
    }
}

/// Whether `sql` references `table` as a whole identifier (word-boundary,
/// case-insensitive token scan — `iot_reading_x` never matches `iot_reading`).
/// Pure; the analytics surface uses it to decide whether the caller's SQL
/// needs the merged distributed readings table registered.
pub fn sql_references_table(sql: &str, table: &str) -> bool {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| token.eq_ignore_ascii_case(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_references_table_matches_whole_identifiers_only() {
        assert!(sql_references_table(
            "SELECT count(*) FROM iot_reading WHERE metric = 'm'",
            "iot_reading"
        ));
        assert!(sql_references_table(
            "select * from IOT_READING",
            "iot_reading"
        ));
        assert!(sql_references_table(
            "SELECT n.node_id FROM nodes n JOIN iot_reading r ON r.node_id = n.node_id",
            "iot_reading"
        ));
        assert!(sql_references_table(
            "SELECT * FROM (SELECT * FROM iot_reading)",
            "iot_reading"
        ));
        assert!(!sql_references_table("SELECT * FROM nodes", "iot_reading"));
        assert!(!sql_references_table(
            "SELECT * FROM iot_readings",
            "iot_reading"
        ));
        // Fail-open by design: a mention inside a string literal
        // over-registers the table (one extra fan-out) rather than
        // under-registering it (the query would fail table-not-found).
        assert!(sql_references_table(
            "SELECT 'iot_reading' AS literal",
            "iot_reading"
        ));
    }
}
