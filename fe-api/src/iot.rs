//! IoT reading ingestion endpoint (FR-4) — see `fe-api/AGENTS.md` §iot-ingest.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_database::handlers::iot_reading::{insert_readings, IotIngestError, IotReadingInput};
use fe_identity::api_token::ApiClaims;
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::limits;
use crate::query_guard;
use crate::server::ApiState;
use crate::types::is_valid_ulid;

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

    let Some(ref db) = state.db_reader else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "iot ingest not available (no db_reader)",
        );
    };

    // Append-only insert — safe off the DB thread (fe-database AGENTS.md §iot-readings).
    match insert_readings(db, &petal_id, &claims.sub, &req.readings).await {
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
    }
}
