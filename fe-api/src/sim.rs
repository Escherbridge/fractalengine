//! Sim control surface (F9/A20): `/api/v1/sim/*` + the MCP `sim_*` tools —
//! see `fe-api/AGENTS.md` §sim-control.
//!
//! One dispatch fn per verb ([`start`], [`stop`], [`status`], [`step`],
//! [`inject_fault`]); REST handlers and MCP arms both call them, so the
//! guard (Owner role → seam presence) lives in exactly one place.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use fe_identity::api_token::ApiClaims;
use fe_runtime::sim_control::{
    SimControlCall, SimControlCallSender, SimControlCommand, SimControlError, SimControlErrorKind,
    SimControlResult, MAX_SIM_STEP,
};
use serde::Deserialize;

use crate::auth::require_role;
use crate::server::ApiState;
use crate::types::ApiResponse;

/// Reply budget for one verb (a start spawns peers; a step may wait out
/// scripted query deadlines and settle barriers).
pub const SIM_CONTROL_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a sim control call failed at the API layer or in the bridge.
#[derive(Debug)]
pub enum SimApiError {
    /// The caller is not Owner.
    Forbidden,
    /// This host has no sim bridge (`ApiState.sim_control_tx` is `None`).
    NotConfigured,
    /// Rejected before reaching the bridge.
    BadRequest(String),
    /// The bridge queue is full.
    Saturated,
    /// The bridge thread is gone.
    Unavailable,
    /// No reply within [`SIM_CONTROL_TIMEOUT`].
    Timeout,
    /// The bridge answered with a typed failure.
    Sim(SimControlError),
}

impl SimApiError {
    /// HTTP status for the REST surface.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotConfigured | Self::Saturated => StatusCode::SERVICE_UNAVAILABLE,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unavailable => StatusCode::BAD_GATEWAY,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Sim(e) => match e.kind {
                SimControlErrorKind::BadRequest => StatusCode::BAD_REQUEST,
                SimControlErrorKind::Conflict | SimControlErrorKind::NotRunning => {
                    StatusCode::CONFLICT
                }
                SimControlErrorKind::Failed => StatusCode::INTERNAL_SERVER_ERROR,
            },
        }
    }

    /// The human-readable reason every surface reports.
    pub fn message(&self) -> String {
        match self {
            Self::Forbidden => "insufficient permissions".into(),
            Self::NotConfigured => {
                "sim control not configured (this host was not built/started as a sim lab)".into()
            }
            Self::BadRequest(m) => m.clone(),
            Self::Saturated => "sim control is busy — too many queued calls, try again".into(),
            Self::Unavailable => "sim control bridge unavailable".into(),
            Self::Timeout => "sim control call timed out".into(),
            Self::Sim(e) => e.message.clone(),
        }
    }
}

/// The guard every verb runs FIRST: Owner role, then seam presence (fail
/// closed). Argument validation comes after, so a non-owner always sees 403.
fn guard<'s>(
    state: &'s ApiState,
    claims: &ApiClaims,
) -> Result<&'s SimControlCallSender, SimApiError> {
    if require_role(claims, "owner").is_err() {
        return Err(SimApiError::Forbidden);
    }
    state
        .sim_control_tx
        .as_ref()
        .ok_or(SimApiError::NotConfigured)
}

/// Round trip: `try_send` (reply-embedded call — never block an API
/// worker), then a bounded wait off the async workers.
async fn round_trip(
    tx: &SimControlCallSender,
    cmd: SimControlCommand,
) -> Result<serde_json::Value, SimApiError> {
    let verb = cmd.verb();
    let (reply_tx, reply_rx) = crossbeam::channel::bounded::<SimControlResult>(1);
    match tx.try_send(SimControlCall { cmd, reply_tx }) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            tracing::warn!(verb, "sim control seam saturated — refusing");
            return Err(SimApiError::Saturated);
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            tracing::warn!(verb, "sim control seam send failed (bridge thread gone)");
            return Err(SimApiError::Unavailable);
        }
    }
    match tokio::task::spawn_blocking(move || reply_rx.recv_timeout(SIM_CONTROL_TIMEOUT)).await {
        Ok(Ok(Ok(payload))) => Ok(payload),
        Ok(Ok(Err(e))) => Err(SimApiError::Sim(e)),
        Ok(Err(crossbeam::channel::RecvTimeoutError::Timeout)) => Err(SimApiError::Timeout),
        Ok(Err(crossbeam::channel::RecvTimeoutError::Disconnected)) => {
            Err(SimApiError::Unavailable)
        }
        Err(e) => {
            tracing::error!(verb, "sim control reply task failed: {e:?}");
            Err(SimApiError::Unavailable)
        }
    }
}

/// Start a session (`script`: scenario object, JSON text, or built-in name).
pub async fn start(
    state: &ApiState,
    claims: &ApiClaims,
    script: serde_json::Value,
) -> Result<serde_json::Value, SimApiError> {
    let tx = guard(state, claims)?;
    if script.is_null() {
        return Err(SimApiError::BadRequest("script is required".into()));
    }
    round_trip(tx, SimControlCommand::Start { script }).await
}

/// Settle, fingerprint, and tear down the live session.
pub async fn stop(state: &ApiState, claims: &ApiClaims) -> Result<serde_json::Value, SimApiError> {
    round_trip(guard(state, claims)?, SimControlCommand::Stop).await
}

/// Snapshot the live session (`{"running": false}` when idle).
pub async fn status(
    state: &ApiState,
    claims: &ApiClaims,
) -> Result<serde_json::Value, SimApiError> {
    round_trip(guard(state, claims)?, SimControlCommand::Status).await
}

/// Execute the next `n` merged actions.
pub async fn step(
    state: &ApiState,
    claims: &ApiClaims,
    n: u64,
) -> Result<serde_json::Value, SimApiError> {
    let tx = guard(state, claims)?;
    let n = u32::try_from(n)
        .ok()
        .filter(|n| (1..=MAX_SIM_STEP).contains(n))
        .ok_or_else(|| SimApiError::BadRequest(format!("n must be 1..={MAX_SIM_STEP}")))?;
    round_trip(tx, SimControlCommand::Step { n }).await
}

/// Apply one `ScriptedEvent` fault now.
pub async fn inject_fault(
    state: &ApiState,
    claims: &ApiClaims,
    event: serde_json::Value,
) -> Result<serde_json::Value, SimApiError> {
    let tx = guard(state, claims)?;
    if !event.is_object() {
        return Err(SimApiError::BadRequest(
            "event must be a ScriptedEvent object".into(),
        ));
    }
    round_trip(tx, SimControlCommand::InjectFault { event }).await
}

// ---------------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------------

/// `POST /api/v1/sim/start` body.
#[derive(Debug, Deserialize)]
pub struct SimStartRequest {
    #[serde(default)]
    pub script: serde_json::Value,
}

/// `POST /api/v1/sim/step` body (`n` defaults to 1).
#[derive(Debug, Deserialize)]
pub struct SimStepRequest {
    #[serde(default = "one")]
    pub n: u64,
}

fn one() -> u64 {
    1
}

/// `POST /api/v1/sim/inject-fault` body.
#[derive(Debug, Deserialize)]
pub struct SimFaultRequest {
    #[serde(default)]
    pub event: serde_json::Value,
}

/// `ApiResponse` envelope with a real HTTP status.
fn respond(result: Result<serde_json::Value, SimApiError>) -> Response {
    match result {
        Ok(payload) => (StatusCode::OK, Json(ApiResponse::success(payload))).into_response(),
        Err(e) => (
            e.status(),
            Json(ApiResponse::<serde_json::Value>::error(e.message())),
        )
            .into_response(),
    }
}

/// POST /api/v1/sim/start — Owner only.
pub async fn post_start(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<SimStartRequest>,
) -> Response {
    respond(start(&state, &claims, req.script).await)
}

/// POST /api/v1/sim/stop — Owner only.
pub async fn post_stop(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> Response {
    respond(stop(&state, &claims).await)
}

/// GET /api/v1/sim/status — Owner only.
pub async fn get_status(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> Response {
    respond(status(&state, &claims).await)
}

/// POST /api/v1/sim/step — Owner only.
pub async fn post_step(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<SimStepRequest>,
) -> Response {
    respond(step(&state, &claims, req.n).await)
}

/// POST /api/v1/sim/inject-fault — Owner only.
pub async fn post_inject_fault(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<SimFaultRequest>,
) -> Response {
    respond(inject_fault(&state, &claims, req.event).await)
}
