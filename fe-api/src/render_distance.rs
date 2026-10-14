//! `petal.render_distance` REST routes + the shared REST/MCP core
//! (p2p_asset_streaming_20260718 S6). Mirrors `terrain.rs`'s
//! set_petal_terrain_core / terrain_write_response shape — see
//! `fe-api/AGENTS.md` §render-distance.

use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use fe_identity::api_token::ApiClaims;

use crate::auth::require_role;
use crate::types::{is_valid_ulid, ApiResponse};

/// PUT /api/v1/petals/:petal_id/render-distance — set the petal's durable
/// residency horizon. RFC-7807-lite body: `{"render_distance": <number>}`.
///
/// RBAC: Editor+ required (re-checked by the DB thread's own Editor+ gate —
/// this is the UI-side courtesy, existence-oracle uniform denial per
/// `fe-api/AGENTS.md` §existence-oracle).
pub async fn set_render_distance(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ))
        .into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id")).into_response();
    }
    if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        return Json(ApiResponse::<serde_json::Value>::error(
            crate::rest::NOT_FOUND_OR_DENIED,
        ))
        .into_response();
    }
    let render_distance = match body.get("render_distance") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_f64() {
            Some(n) => Some(n as f32),
            None => {
                return RenderDistanceWriteError::InvalidValue(
                    "render_distance must be a number or null".to_string(),
                )
                .into_response();
            }
        },
    };

    render_distance_write_response(
        &petal_id,
        set_render_distance_core(&state, &petal_id, render_distance).await,
    )
}

/// GET /api/v1/petals/:petal_id/render-distance — read the petal's durable
/// `render_distance` (`null` = inherit the global setting). RBAC: Viewer+.
pub async fn get_render_distance(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id"));
    }
    if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        return Json(ApiResponse::<serde_json::Value>::error(
            crate::rest::NOT_FOUND_OR_DENIED,
        ));
    }
    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::<serde_json::Value>::error(
            "direct DB access not available",
        ));
    };
    let mut res = match db
        .query("SELECT render_distance FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Json(ApiResponse::<serde_json::Value>::error(format!(
                "query failed: {e}"
            )));
        }
    };
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    let render_distance = rows
        .first()
        .and_then(|r| r.get("render_distance"))
        .and_then(|v| v.as_f64());
    Json(ApiResponse::success(serde_json::json!({
        "petal_id": petal_id,
        "render_distance": render_distance,
    })))
}

/// DELETE /api/v1/petals/:petal_id/render-distance — clear the petal's
/// override (inherit the global setting). RBAC: Editor+.
pub async fn delete_render_distance(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ))
        .into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<serde_json::Value>::error("invalid petal_id")).into_response();
    }
    if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        return Json(ApiResponse::<serde_json::Value>::error(
            crate::rest::NOT_FOUND_OR_DENIED,
        ))
        .into_response();
    }
    render_distance_write_response(
        &petal_id,
        set_render_distance_core(&state, &petal_id, None).await,
    )
}

/// How long a render-distance mutation waits for the DB thread's echo
/// (mirrors `terrain.rs::TERRAIN_WRITE_TIMEOUT_SECS`).
const RENDER_DISTANCE_WRITE_TIMEOUT_SECS: u64 = 10;

/// Why a petal-render-distance mutation did not land; never carries DB internals.
#[derive(Debug)]
pub(crate) enum RenderDistanceWriteError {
    /// Out of range, non-finite, or the wrong JSON shape.
    InvalidValue(String),
    /// DB thread unreachable, failed, or answered with someone else's reply
    /// (including an Editor+ denial from the DB thread's own gate).
    WriteFailed,
    /// No reply within [`RENDER_DISTANCE_WRITE_TIMEOUT_SECS`].
    TimedOut,
}

impl RenderDistanceWriteError {
    fn status(&self) -> StatusCode {
        match self {
            Self::InvalidValue(_) => StatusCode::BAD_REQUEST,
            Self::WriteFailed => StatusCode::BAD_GATEWAY,
            Self::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::InvalidValue(detail) => format!("invalid render_distance: {detail}"),
            Self::WriteFailed => "render distance write failed".to_string(),
            Self::TimedOut => "render distance write timed out (db thread)".to_string(),
        }
    }
}

impl IntoResponse for RenderDistanceWriteError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(ApiResponse::<serde_json::Value>::error(self.message())),
        )
            .into_response()
    }
}

fn render_distance_write_response(
    petal_id: &str,
    result: Result<Option<f32>, RenderDistanceWriteError>,
) -> Response {
    match result {
        Ok(render_distance) => Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "render_distance": render_distance,
        })))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// Shared REST PUT/DELETE + MCP `set_petal_render_distance` core (caller has
/// already authorized the petal): sends a correlated `SetPetalRenderDistance`
/// to the DB thread (the single writer, and the Editor+ enforcement point —
/// `CallerAuth::Identified` carries the ALREADY-validated API role, same
/// pattern as `terrain.rs::set_petal_terrain_core`) and returns the value it
/// echoes.
pub(crate) async fn set_render_distance_core(
    state: &crate::server::ApiState,
    petal_id: &str,
    render_distance: Option<f32>,
) -> Result<Option<f32>, RenderDistanceWriteError> {
    use fe_runtime::messages::{ApiCommand, CallerAuth, DbCommand, DbResult};

    if let Some(v) = render_distance {
        if !v.is_finite() || v < 0.0 {
            return Err(RenderDistanceWriteError::InvalidValue(
                "render_distance must be finite and >= 0".to_string(),
            ));
        }
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::SetPetalRenderDistance {
        petal_id: petal_id.to_string(),
        render_distance,
        // The API layer already authorized Editor+ above; the DB thread
        // re-validates with the role an already-identified caller carries
        // (T5 — the same trust boundary `terrain.rs`/`visibility_groups`
        // REST routes use).
        auth: CallerAuth::Identified {
            did: "api".to_string(),
            role: "editor".to_string(),
        },
        correlation_id: Some(correlation_id.clone()),
    };
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .is_err()
    {
        tracing::warn!(
            petal_id,
            "render distance write: API command channel closed"
        );
        return Err(RenderDistanceWriteError::WriteFailed);
    }
    let ours = |reply_id: &str, reply_petal: &str| {
        reply_id == correlation_id.as_str() && reply_petal == petal_id
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(RENDER_DISTANCE_WRITE_TIMEOUT_SECS),
        reply_rx,
    )
    .await
    {
        Ok(Ok(DbResult::PetalRenderDistanceLoaded {
            petal_id: reply_petal,
            render_distance,
            correlation_id: Some(reply_id),
        })) if ours(&reply_id, &reply_petal) => Ok(render_distance),
        Ok(Ok(DbResult::RequestFailed {
            error,
            correlation_id: reply_id,
        })) if reply_id == correlation_id => {
            tracing::warn!(petal_id, error = %error, "render distance write failed on the DB thread");
            Err(RenderDistanceWriteError::WriteFailed)
        }
        Ok(Ok(other)) => {
            tracing::warn!(
                petal_id,
                correlation_id,
                ?other,
                "render distance write: reply correlation mismatch — refused"
            );
            Err(RenderDistanceWriteError::WriteFailed)
        }
        Ok(Err(_)) => {
            tracing::warn!(petal_id, "render distance write: DB reply channel dropped");
            Err(RenderDistanceWriteError::WriteFailed)
        }
        Err(_) => {
            tracing::warn!(petal_id, "render distance write: DB thread timed out");
            Err(RenderDistanceWriteError::TimedOut)
        }
    }
}
