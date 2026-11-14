//! Read-only listing of a petal's quarantined imported op-log history (see `AGENTS.md` §petal-snapshot).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

/// Rows returned when the caller names no limit.
const DEFAULT_LIMIT: u32 = 100;
/// Largest page a caller may ask for.
const MAXIMUM_LIMIT: u32 = 500;

/// Summary columns only; payloads stay in the export.
const HISTORY_QUERY: &str = "SELECT source_id, import_id, imported_by, op_type, node_id, lamport_clock, hlc_timestamp, correlation_id \
FROM op_log_imported WHERE import_petal_id = $petal AND lamport_clock > $after ORDER BY lamport_clock ASC LIMIT $limit";

/// Cursor parameters of the imported-history listing.
#[derive(Debug, Default, Deserialize)]
pub struct ImportedHistoryParams {
    /// Exclusive lamport cursor (default 0).
    pub after_lamport: Option<u64>,
    /// Page size, clamped to 1..=500 (default 100).
    pub limit: Option<u32>,
}

/// GET /api/v1/petals/{petal_id}/imported-history — quarantined imported op-log rows, summary only. Manager+.
pub async fn imported_history(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(params): Query<ImportedHistoryParams>,
) -> Response {
    if !is_valid_ulid(&petal_id) {
        return (StatusCode::BAD_REQUEST, "invalid petal_id").into_response();
    }
    let resolved = crate::rest::resolve_petal_scope(&state, &petal_id).await;
    if crate::petal_snapshot::manager_covers(&claims, resolved).is_none() {
        return crate::petal_snapshot::denied();
    }
    let Some(reader) = state.db_reader.as_ref() else {
        return unavailable();
    };

    let after = i64::try_from(params.after_lamport.unwrap_or(0)).unwrap_or(i64::MAX);
    let limit = i64::from(params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAXIMUM_LIMIT));
    let rows = match reader
        .query(HISTORY_QUERY)
        .bind(("petal", petal_id.clone()))
        .bind(("after", after))
        .bind(("limit", limit))
        .await
    {
        Ok(mut response) => response.take::<Vec<Value>>(0),
        Err(error) => Err(error),
    };
    let entries = match rows {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(petal_id = %petal_id, "imported history read failed: {error}");
            return unavailable();
        }
    };

    let next_after_lamport = entries.last().and_then(|entry| entry.get("lamport_clock")).cloned().unwrap_or(Value::Null);
    Json(ApiResponse::success(json!({ "entries": entries, "next_after_lamport": next_after_lamport }))).into_response()
}

/// A JSON 503 for a missing reader or a failed read.
fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(ApiResponse::<Value>::error("database unavailable"))).into_response()
}
