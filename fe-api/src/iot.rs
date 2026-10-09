//! IoT reading ingestion endpoint (FR-4) — see `fe-api/AGENTS.md` §iot-ingest.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_database::handlers::iot_reading::{
    insert_readings_with_replication, IotIngestError, IotReadingInput,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::limits;
use crate::query_guard;
use crate::server::ApiState;
use crate::types::is_valid_ulid;

/// Bound on the DB-thread round-trip of the no-`db_reader` ingest fallback
/// (F24) — generous over any realistic batch write, bounded so a wedged DB
/// thread can never pin an API worker (the same budget class as the
/// analytics/query timeouts).
const FALLBACK_TIMEOUT_SECS: u64 = 10;

/// Batch ingestion request body.
#[derive(Debug, Deserialize)]
pub struct IotIngestRequest {
    pub readings: Vec<IotReadingInput>,
}

/// Structured JSON error with a real HTTP status (export.rs precedent).
fn err(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

/// POST /api/v1/petals/:petal_id/iot/readings — authenticated batch ingest.
///
/// Write path: `db_reader` when configured (the §iot-readings append-only
/// exception), else the DB-thread seam (`DbCommand::InsertIotReadings`, F24)
/// — never a 503. The guard pipeline (Editor+ role → ULID → petal scope →
/// token containment → per-DID rate limit → batch caps) runs identically on
/// both paths, before any write.
pub async fn ingest_readings(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(req): Json<IotIngestRequest>,
) -> Response {
    // Ingestion is a write: Editor+ (deny-by-default, export.rs guard order).
    if require_role(&claims, "editor").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&petal_id) {
        return err(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    let Some(scope) = crate::rest::resolve_petal_scope(&state, &petal_id).await else {
        return err(StatusCode::NOT_FOUND, "unknown petal");
    };
    if require_scope(&claims, &scope).is_err() {
        tracing::warn!(petal_id, sub = %claims.sub, "iot ingest denied: token scope does not cover petal");
        return err(StatusCode::FORBIDDEN, "insufficient scope");
    }

    let rate_key = format!("iot:{}", claims.sub);
    if let Err(e) = query_guard::check_rate_limit(
        &state,
        &rate_key,
        limits::IOT_INGEST_RATE_PER_SEC,
        "10 ingest requests/sec",
    )
    .await
    {
        return err(StatusCode::TOO_MANY_REQUESTS, &e);
    }

    if req.readings.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty readings batch");
    }
    if req.readings.len() > limits::IOT_INGEST_MAX_READINGS {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "batch exceeds cap ({} readings max per request)",
                limits::IOT_INGEST_MAX_READINGS
            ),
        );
    }

    // A11: publish each accepted row to the verse's replica. The API thread
    // writes on `db_reader` (append-only exception, §iot-readings), so it
    // carries its own emit seam: the shared blob store supplies the row's
    // content hash and `replication_tx` is the DB→sync bridge. Both are absent
    // in a replication-disabled deployment — the rows are still durable, they
    // simply are not published.
    let verse_id = fe_database::parse_scope(&scope)
        .ok()
        .map(|parts| parts.verse_id);

    match state.db_reader.as_ref() {
        // Append-only insert — safe off the DB thread (fe-database AGENTS.md §iot-readings).
        Some(db) => match insert_readings_with_replication(
            db,
            &petal_id,
            verse_id.as_deref(),
            &claims.sub,
            &req.readings,
            state.blob_store.as_ref(),
            state.replication_tx.as_ref(),
        )
        .await
        {
            Ok(accepted) => (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "accepted": accepted })),
            )
                .into_response(),
            Err(
                e @ (IotIngestError::UnknownAnchor(_)
                | IotIngestError::InvalidTimestamp(_)
                | IotIngestError::EmptyMetric),
            ) => err(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()),
            Err(IotIngestError::Db(e)) => {
                tracing::error!(petal_id, error = %e, "iot ingest DB write failed");
                err(StatusCode::BAD_GATEWAY, "reading write failed")
            }
        },
        // F24: no `db_reader` (the SurrealKV per-handle file lock rejects the
        // second in-process connection on Windows while the DB-thread writer
        // lives — the deployment platform). Fall back to the DB-thread seam
        // (`DbCommand::InsertIotReadings`, F7): the write happens ON the DB
        // thread, which is MORE aligned with the single-writer rule than the
        // `db_reader` append-only exception. Every guard above (Editor+ role,
        // petal-scope containment, per-DID rate limit, batch caps) has already
        // run on THIS path, and `claims.sub` rides the command so the
        // DB-thread handler's `source_did` context is the acting caller's.
        // The DB-thread arm rides `insert_readings_with_replication`, so the
        // per-row ReplicationEvents fire exactly as on the direct path.
        None => {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            // DEC-C13: correlate the reply. Without it a 10s timeout left
            // our closed entry in the family queue and our late reply landed
            // on the NEXT caller (fe-runtime src/AGENTS.md
            // §api-reply-correlation — routing gives a correlated reply only
            // to its own entry; the check below is defense in depth).
            let correlation_id = ulid::Ulid::new().to_string();
            let cmd = DbCommand::InsertIotReadings {
                petal_id: petal_id.clone(),
                verse_id,
                source_did: claims.sub.clone(),
                readings: req.readings,
                correlation_id: Some(correlation_id.clone()),
            };
            if state
                .api_cmd_tx
                .send(ApiCommand::DbRequest { cmd, reply_tx })
                .is_err()
            {
                tracing::warn!(petal_id, "iot ingest fallback: API command channel closed");
                return err(StatusCode::BAD_GATEWAY, "db thread unavailable");
            }
            // A reply is ours only if its echoed id AND petal match (DEC-C13
            // defense in depth over the router's correlated delivery).
            let ours = |reply_id: Option<&str>, reply_petal: &str| {
                reply_id == Some(correlation_id.as_str()) && reply_petal == petal_id
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(FALLBACK_TIMEOUT_SECS),
                reply_rx,
            )
            .await
            {
                Ok(Ok(DbResult::IotReadingsInserted {
                    written,
                    petal_id: reply_petal,
                    correlation_id: reply_id,
                })) if ours(reply_id.as_deref(), &reply_petal) => (
                    StatusCode::OK,
                    Json(serde_json::json!({ "ok": true, "accepted": written })),
                )
                    .into_response(),
                Ok(Ok(DbResult::IotReadingsRejected {
                    reason,
                    petal_id: reply_petal,
                    correlation_id: reply_id,
                })) if ours(reply_id.as_deref(), &reply_petal) => {
                    err(StatusCode::UNPROCESSABLE_ENTITY, &reason.to_string())
                }
                // Another caller's reply (correlation or petal mismatch): never
                // report it as ours — no accepted count or 422 detail leaks.
                Ok(Ok(
                    DbResult::IotReadingsInserted { .. } | DbResult::IotReadingsRejected { .. },
                )) => {
                    tracing::warn!(
                        petal_id,
                        correlation_id,
                        "iot ingest fallback: reply correlation mismatch — refused"
                    );
                    err(StatusCode::BAD_GATEWAY, "reading write failed")
                }
                // Family-blind `Error` cannot be correlated (known sharp edge,
                // fe-runtime src/AGENTS.md §api-reply-correlation).
                Ok(Ok(DbResult::Error(e))) => {
                    tracing::error!(petal_id, error = %e, "iot ingest DB-thread write failed");
                    err(StatusCode::BAD_GATEWAY, "reading write failed")
                }
                Ok(Ok(other)) => {
                    tracing::warn!(?other, "iot ingest fallback: unexpected reply family");
                    err(StatusCode::BAD_GATEWAY, "reading write failed")
                }
                Ok(Err(_)) => {
                    tracing::warn!(petal_id, "iot ingest fallback: DB reply channel dropped");
                    err(StatusCode::BAD_GATEWAY, "reading write failed")
                }
                Err(_) => {
                    tracing::warn!(petal_id, "iot ingest fallback: DB thread timed out");
                    err(
                        StatusCode::GATEWAY_TIMEOUT,
                        "reading write timed out (db thread)",
                    )
                }
            }
        }
    }
}
