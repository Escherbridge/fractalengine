//! `POST /api/v1/debug/screenshot` (dynamic pose capture) + `GET
//! /api/v1/debug/camera` — see AGENTS.md §screenshot, §camera-state.
//!
//! A SEPARATE channel and gate from the plain `GET` path in `screenshot.rs`
//! (fe-runtime/src/AGENTS.md §vision-bridge) on the SAME correlated-bridge
//! envelope and round trip (§correlated-bridge, `bridge_client.rs`).

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
    VisionOutput, VisionRefusal,
};

use crate::bridge_client::{round_trip, RoundTripError};
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
                VisionFailureClass::Denied => StatusCode::NOT_FOUND,
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

    /// MCP tool-error text: the message prefixed with its stable `code` (and
    /// `field`) so an agent gets the same machine-readable refusal REST puts
    /// in the JSON body, e.g. `geo_out_of_range (pose.geo.lat): ...`. The
    /// scope denial is the exception: it stays the bare §existence-oracle
    /// text, byte-identical on every door (plain/posed, REST/MCP).
    pub fn coded_message(&self) -> String {
        if self.status() == StatusCode::NOT_FOUND {
            return self.message();
        }
        match (self.code(), self.field()) {
            (Some(code), Some(field)) => format!("{code} ({field}): {}", self.message()),
            (Some(code), None) => format!("{code}: {}", self.message()),
            (None, _) => self.message(),
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
// Bridge round-trip (the shared `bridge_client::round_trip`)
// ---------------------------------------------------------------------------

/// Ask the Bevy host to run one vision op on behalf of `caller_scope` (the
/// token's scope — THE one authorization input both doors pass; the host
/// checks it against the active verse and any focus target at capture time,
/// SEC-ADDENDUM). Never blocks the async worker: a full bridge is a busy
/// refusal, a silent host a timeout, and a reply echoing a foreign
/// correlation id is rejected.
pub async fn request_vision(
    state: &ApiState,
    caller_scope: &str,
    op: VisionOp,
) -> Result<VisionOutput, VisionError> {
    let Some(bridge) = state.screenshot.vision.as_ref() else {
        return Err(VisionError::Unavailable(
            "vision unavailable: this host has no window",
        ));
    };
    let timeout = if matches!(op, VisionOp::Capture(_)) {
        state.screenshot.reply_timeout + CAPTURE_SETTLE_WALL_CLOCK_CAP
    } else {
        state.screenshot.reply_timeout
    };
    let transport_failure = |code: &'static str, message: &str| {
        VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Failed,
            code,
            message: message.to_string(),
        })
    };
    let reply = round_trip(bridge, caller_scope, op, timeout, "vision")
        .await
        .map_err(|error| match error {
            RoundTripError::Saturated => {
                VisionError::Unavailable("vision bridge busy — retry shortly")
            }
            RoundTripError::HostGone => {
                VisionError::Unavailable("vision unavailable: render host stopped")
            }
            RoundTripError::TimedOut(waited) => VisionError::TimedOut(waited),
            RoundTripError::Uncorrelated => {
                transport_failure("uncorrelated_reply", "uncorrelated reply")
            }
            RoundTripError::Dropped => transport_failure("request_dropped", "vision request dropped"),
        })?;
    reply.map_err(VisionError::Failure)
}

/// Run one posed capture: acquire the single-flight slot (409, bridge
/// untouched, when a capture is already in progress) then cross the bridge.
/// Shared by the REST handler and the MCP `screenshot` handler.
pub(crate) async fn run_capture(
    state: &ApiState,
    caller_scope: &str,
    spec: ValidCaptureSpec,
) -> Result<(Vec<u8>, ResolvedPose), VisionError> {
    let _permit = state
        .screenshot
        .capture_slot
        .clone()
        .try_acquire_owned()
        .map_err(|_| VisionError::Busy)?;
    match request_vision(state, caller_scope, VisionOp::Capture(spec)).await? {
        VisionOutput::Capture { png, pose } => Ok((png, pose)),
        VisionOutput::Camera(_) => Err(VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Failed,
            code: "unexpected_reply",
            message: "vision bridge returned a camera reply for a capture request".to_string(),
        })),
    }
}

/// The resolved pose as response metadata JSON (the `X-FE-Capture-Pose`
/// header and the MCP text part): the pose plus `include_ui` (UI-ADDENDUM
/// item 3 — derived from the resolved mode, so it can never disagree).
pub(crate) fn pose_metadata_json(pose: &ResolvedPose) -> String {
    let mut value = serde_json::to_value(pose).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "include_ui".to_string(),
            serde_json::Value::Bool(pose.mode == fe_runtime::vision::CaptureMode::Window),
        );
    }
    value.to_string()
}

/// The ONE parse-and-validate path for a capture request, shared by both
/// doors (REST body bytes, MCP arguments): a structural serde failure
/// (including unknown fields — `deny_unknown_fields`) is `malformed_body`,
/// everything else is [`CaptureSpec::validate`]'s typed table.
pub(crate) fn validate_capture_request(
    parsed: Result<CaptureSpec, serde_json::Error>,
) -> Result<ValidCaptureSpec, VisionRefusal> {
    parsed
        .map_err(|error| VisionRefusal {
            code: "malformed_body",
            field: None,
            message: format!("malformed request body: {error}"),
        })?
        .validate()
}

/// REST door: a body over [`MAX_BODY_BYTES`] is `malformed_body`, an empty
/// body is a bare default (window, no pose) capture, anything else goes
/// through [`validate_capture_request`].
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
    if body.is_empty() {
        return CaptureSpec::default().validate();
    }
    validate_capture_request(serde_json::from_slice(body))
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
    match run_capture(&state, &claims.scope, spec).await {
        Ok((png, pose)) => {
            let pose_json = pose_metadata_json(&pose);
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
    match request_vision(&state, &claims.scope, VisionOp::ReadCamera).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's uniform denial and REST's §existence-oracle text are one
    /// string, so a vision 404 is indistinguishable from any other.
    #[test]
    fn host_denial_matches_the_rest_existence_oracle_text() {
        assert_eq!(fe_runtime::vision::DENIED_MESSAGE, crate::rest::NOT_FOUND_OR_DENIED);
        assert_eq!(
            fe_runtime::screenshot::SCREENSHOT_SCOPE_DENIED,
            crate::rest::NOT_FOUND_OR_DENIED
        );
        let error = VisionError::Failure(fe_runtime::vision::denied());
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
        assert_eq!(error.message(), crate::rest::NOT_FOUND_OR_DENIED);
    }

    /// MCP error text: a typed refusal carries its `code` (and `field`) like
    /// the REST body, but the scope denial stays the bare oracle text, so the
    /// MCP posed door answers exactly like the MCP plain door.
    #[test]
    fn coded_message_prefixes_refusals_but_never_the_denial() {
        let refusal = VisionError::Refusal(VisionRefusal {
            code: "geo_out_of_range",
            field: Some("pose.geo.lat"),
            message: "lat out of range".to_string(),
        });
        let busy = VisionError::Failure(VisionFailure {
            status_class: VisionFailureClass::Busy,
            code: "capture_in_progress",
            message: "busy".to_string(),
        });
        let cases = [
            (refusal, "geo_out_of_range (pose.geo.lat): lat out of range".to_string()),
            (busy, "capture_in_progress: busy".to_string()),
            (
                VisionError::Failure(fe_runtime::vision::denied()),
                crate::rest::NOT_FOUND_OR_DENIED.to_string(),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.coded_message(), expected);
        }
    }
}
