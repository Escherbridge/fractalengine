//! `POST /api/v1/debug/screenshot` (dynamic pose capture) + `GET
//! /api/v1/debug/camera` — see AGENTS.md §screenshot, §camera-state.
//!
//! This is deliberately a SEPARATE bridge/gate pair from the plain `GET`
//! path in `screenshot.rs` (fe-runtime/src/AGENTS.md §vision-bridge): the
//! GET route's bytes and behaviour stay byte-for-byte unchanged by anything
//! here.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::vision::{
    CaptureSpec, ResolvedPose, ValidCaptureSpec, VisionFailure, VisionFailureClass, VisionOp,
    VisionOutput, VisionRefusal, VisionRequest,
};

use crate::operator_gate::{GateRefusal, OperatorGate};
use crate::screenshot::SCREENSHOT_GATE;
use crate::server::ApiState;
use crate::types::ApiResponse;

/// `POST`'s raw-body size cap (spec §5.2's `malformed_body` row) — a dynamic
/// capture request is a handful of floats, never kilobytes.
const MAX_BODY_BYTES: usize = 4096;

/// Extra wall-clock budget a posed capture's settle may need on top of the
/// plain reply timeout (spec §5.7: "5 s + settle wall-clock cap (<= 25 s)").
const CAPTURE_SETTLE_WALL_CLOCK_CAP: Duration = Duration::from_secs(20);

/// The `X-FE-Capture-Pose` response header name (spec §5.2).
const POSE_HEADER: &str = "x-fe-capture-pose";

/// The camera-read-back gate: same env var and default as screenshots (one
/// operator switch), its own rate key and a 10/s budget so the closed loop
/// `get_camera -> screenshot(pose)` never burns the 1/s capture budget —
/// AGENTS.md §camera-state, §operator-gate.
pub const CAMERA_STATE_GATE: OperatorGate = OperatorGate {
    opt_in_env: crate::screenshot::SCREENSHOT_ALLOW_ENV,
    endpoint: "camera state endpoint",
    enabled_warning: "GET /api/v1/debug/camera and the MCP `get_camera` tool admit Owner tokens \
                      — the live camera pose, active hierarchy and projection info. Set \
                      FE_API_SCREENSHOT_ALLOW=0 to disable (shares the screenshot opt-in)",
    disabled_message: "camera state endpoint is disabled on this node",
    rate_key_prefix: "camera",
    rate_per_sec: 10,
    rate_label: "10 camera reads/sec",
    default_on: true,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a vision request was refused or failed; each maps to one HTTP status
/// (the same shape as `screenshot::ScreenshotError`).
#[derive(Debug, Clone, PartialEq)]
pub enum VisionError {
    /// The shared operator gate refused (opt-in, Owner or rate).
    Refused(GateRefusal),
    /// A static validation check failed (`CaptureSpec::validate`) — 400.
    Refusal(VisionRefusal),
    /// A posed capture is already in flight (the single-flight slot) — 409.
    Busy,
    Unavailable(&'static str),
    TimedOut(Duration),
    /// A dynamic (Bevy-side) refusal or failure.
    Failure(VisionFailure),
}

impl From<GateRefusal> for VisionError {
    fn from(refusal: GateRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl VisionError {
    /// HTTP status for the REST surface.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Refused(GateRefusal::Disabled(_) | GateRefusal::InsufficientRole) => {
                StatusCode::FORBIDDEN
            }
            Self::Refused(GateRefusal::RateLimited(_)) => StatusCode::TOO_MANY_REQUESTS,
            Self::Refusal(_) => StatusCode::BAD_REQUEST,
            Self::Busy => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::Failure(failure) => match failure.status_class {
                VisionFailureClass::Unresolvable | VisionFailureClass::NoWindow => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                VisionFailureClass::Busy => StatusCode::CONFLICT,
                VisionFailureClass::Failed => StatusCode::INTERNAL_SERVER_ERROR,
            },
        }
    }

    /// The stable machine code, when this refusal has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refusal(refusal) => Some(refusal.code),
            Self::Busy => Some("capture_in_progress"),
            Self::Failure(failure) => Some(failure.code),
            Self::Refused(_) | Self::Unavailable(_) | Self::TimedOut(_) => None,
        }
    }

    /// The dotted field path, when this refusal is field-scoped.
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::Refusal(refusal) => refusal.field,
            _ => None,
        }
    }

    /// Client-facing text (REST error body and MCP tool error).
    pub fn message(&self) -> String {
        match self {
            Self::Refused(refusal) => refusal.message(),
            Self::Refusal(refusal) => refusal.message.clone(),
            Self::Busy => "a posed capture is already in progress".to_string(),
            Self::Unavailable(reason) => (*reason).to_string(),
            Self::TimedOut(waited) => format!(
                "vision request timed out — the window did not answer within {:.1}s",
                waited.as_secs_f32()
            ),
            Self::Failure(failure) => failure.message.clone(),
        }
    }
}

fn gate_status(refusal: &GateRefusal) -> StatusCode {
    match refusal {
        GateRefusal::Disabled(_) | GateRefusal::InsufficientRole => StatusCode::FORBIDDEN,
        GateRefusal::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
    }
}

/// The REST/MCP error body: `{ok:false, error, code?, field?}`.
#[derive(Debug, serde::Serialize)]
struct VisionErrorBody {
    ok: bool,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
}

impl VisionErrorBody {
    fn gate(refusal: &GateRefusal) -> Self {
        Self {
            ok: false,
            error: refusal.message(),
            code: None,
            field: None,
        }
    }

    fn refusal(refusal: &VisionRefusal) -> Self {
        Self {
            ok: false,
            error: refusal.message.clone(),
            code: Some(refusal.code),
            field: refusal.field,
        }
    }

    fn error(error: &VisionError) -> Self {
        Self {
            ok: false,
            error: error.message(),
            code: error.code(),
            field: error.field(),
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge round-trip (mirrors `screenshot::request_capture`)
// ---------------------------------------------------------------------------

/// Ask the Bevy host to run one vision op. Never blocks the async worker: a
/// full bridge is a busy refusal, a silent host a timeout, and a reply
/// echoing a foreign correlation id is rejected.
pub async fn request_vision(state: &ApiState, op: VisionOp) -> Result<VisionOutput, VisionError> {
    let Some(bridge) = state.screenshot.vision.as_ref() else {
        return Err(VisionError::Unavailable(
            "vision unavailable: this host has no window",
        ));
    };
    let is_capture = matches!(op, VisionOp::Capture(_));
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let request = VisionRequest {
        correlation_id: correlation_id.clone(),
        op,
        reply_tx,
    };
    match bridge.try_send(request) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            tracing::warn!("vision bridge saturated — refusing");
            return Err(VisionError::Unavailable(
                "vision bridge busy — retry shortly",
            ));
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            tracing::warn!("vision bridge closed (Bevy host gone)");
            return Err(VisionError::Unavailable(
                "vision unavailable: render host stopped",
            ));
        }
    }
    let timeout = if is_capture {
        state.screenshot.reply_timeout + CAPTURE_SETTLE_WALL_CLOCK_CAP
    } else {
        state.screenshot.reply_timeout
    };
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(reply)) if reply.correlation_id == correlation_id => {
            reply.result.map_err(VisionError::Failure)
        }
        Ok(Ok(_)) => Err(VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Failed,
            code: "uncorrelated_reply",
            message: "uncorrelated reply".to_string(),
        })),
        Ok(Err(_)) => Err(VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Failed,
            code: "request_dropped",
            message: "vision request dropped".to_string(),
        })),
        Err(_) => Err(VisionError::TimedOut(timeout)),
    }
}

/// Run one posed capture: acquire the single-flight slot (409, bridge
/// untouched, when a capture is already in progress) then cross the bridge.
/// Shared by the REST handler and the MCP `screenshot` handler.
pub(crate) async fn run_capture(
    state: &ApiState,
    spec: ValidCaptureSpec,
) -> Result<(Vec<u8>, ResolvedPose), VisionError> {
    let _permit = state
        .screenshot
        .capture_slot
        .clone()
        .try_acquire_owned()
        .map_err(|_| VisionError::Busy)?;
    match request_vision(state, VisionOp::Capture(spec)).await? {
        VisionOutput::Capture { png, pose } => Ok((png, pose)),
        VisionOutput::Camera(_) => Err(VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Failed,
            code: "unexpected_reply",
            message: "vision bridge returned a camera reply for a capture request".to_string(),
        })),
    }
}

/// Parse (or default) + validate the POST body in one step: a body over
/// [`MAX_BODY_BYTES`] or that fails to parse as [`CaptureSpec`] (including
/// unknown fields — `deny_unknown_fields`) is `malformed_body`; an empty body
/// is a bare default (window, no pose) capture.
fn parse_and_validate(body: &[u8]) -> Result<ValidCaptureSpec, VisionRefusal> {
    if body.len() > MAX_BODY_BYTES {
        return Err(VisionRefusal {
            code: "malformed_body",
            field: None,
            message: format!(
                "request body too large: {} bytes (max {MAX_BODY_BYTES})",
                body.len()
            ),
        });
    }
    let spec: CaptureSpec = if body.is_empty() {
        CaptureSpec::default()
    } else {
        serde_json::from_slice(body).map_err(|error| VisionRefusal {
            code: "malformed_body",
            field: None,
            message: format!("malformed request body: {error}"),
        })?
    };
    spec.validate()
}

// ---------------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------------

/// `POST /api/v1/debug/screenshot` — a dynamic posed capture (spec §5.2).
///
/// Gate order (AGENTS.md §screenshot, spec §5.7): opt-in → Owner (typed) →
/// 1/s per DID → static validation (400) → single-flight slot (409) →
/// bridge. Validation runs AFTER the gate on purpose — an unauthenticated or
/// un-opted caller never learns the parameter grammar.
pub async fn post_screenshot(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: Bytes,
) -> Response {
    if let Err(refusal) = SCREENSHOT_GATE
        .admit(state.screenshot.allowed, &state, &claims)
        .await
    {
        return (gate_status(&refusal), Json(VisionErrorBody::gate(&refusal))).into_response();
    }
    let spec = match parse_and_validate(&body) {
        Ok(spec) => spec,
        Err(refusal) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(VisionErrorBody::refusal(&refusal)),
            )
                .into_response();
        }
    };
    match run_capture(&state, spec).await {
        Ok((png, pose)) => {
            let pose_json = serde_json::to_string(&pose).unwrap_or_else(|_| "{}".to_string());
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/png")
                .header(header::CACHE_CONTROL, "no-store");
            if let Ok(value) = header::HeaderValue::from_str(&pose_json) {
                builder = builder.header(POSE_HEADER, value);
            } else {
                tracing::warn!("resolved pose did not encode as an ASCII header value");
            }
            builder
                .body(Body::from(png))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) => (error.status(), Json(VisionErrorBody::error(&error))).into_response(),
    }
}

/// `GET /api/v1/debug/camera` — the current main-camera pose, active
/// hierarchy and projection (spec §5.6); headless-safe.
pub async fn get_camera(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> Response {
    if let Err(refusal) = CAMERA_STATE_GATE
        .admit(state.screenshot.allowed, &state, &claims)
        .await
    {
        return (gate_status(&refusal), Json(VisionErrorBody::gate(&refusal))).into_response();
    }
    match request_vision(&state, VisionOp::ReadCamera).await {
        Ok(VisionOutput::Camera(camera_state)) => {
            (StatusCode::OK, Json(ApiResponse::success(camera_state))).into_response()
        }
        Ok(VisionOutput::Capture { .. }) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(VisionErrorBody {
                ok: false,
                error: "vision bridge returned a capture reply for a camera request".to_string(),
                code: None,
                field: None,
            }),
        )
            .into_response(),
        Err(error) => (error.status(), Json(VisionErrorBody::error(&error))).into_response(),
    }
}
