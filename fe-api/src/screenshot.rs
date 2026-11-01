//! `GET /api/v1/debug/screenshot` + the MCP `screenshot` tool's shared core:
//! default-on, Owner-gated, scope-confined agent visual verification — see
//! AGENTS.md §screenshot.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::screenshot::{CapturePrimaryWindow, ScreenshotRequestSender};

use crate::bridge_client::{round_trip, RoundTripError};
use crate::operator_gate::{GateRefusal, OperatorGate};
use crate::server::ApiState;
use crate::types::ApiResponse;

/// Env var for the screenshot surface: unset/`1` = on, `0` = off, anything
/// else = off (fail closed) — agent_vision_20261031 DD-1.
pub const SCREENSHOT_ALLOW_ENV: &str = "FE_API_SCREENSHOT_ALLOW";

/// How long the API waits for the Bevy host to capture and encode a frame.
pub const DEFAULT_SCREENSHOT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The screenshot gate: opt-in (ON unless `=0` — agent_vision_20261031 DD-1)
/// → Owner → 1/s per DID; the host then confines it to the caller's scope — AGENTS.md
/// §screenshot, §operator-gate.
pub const SCREENSHOT_GATE: OperatorGate = OperatorGate {
    opt_in_env: SCREENSHOT_ALLOW_ENV,
    endpoint: "screenshot endpoint",
    enabled_warning: "GET/POST /api/v1/debug/screenshot and the MCP `screenshot` tool admit Owner \
                      tokens whose scope covers the open verse. A capture shows the whole \
                      window of that verse; a posed scene capture can also look anywhere in \
                      the active petal without the user noticing. Set \
                      FE_API_SCREENSHOT_ALLOW=0 to disable, or run only on a single-tenant node \
                      you are actively driving with an agent",
    disabled_message: "screenshot endpoint is disabled on this node",
    rate_key_prefix: "screenshot",
    rate_per_sec: 1,
    rate_label: "1 screenshot/sec",
    // agent_vision_20261031 DD-1: unset admits an Owner; `=0` refuses.
    default_on: true,
};

/// The screenshot + vision surface as wired on this host (`ApiState.screenshot`).
#[derive(Clone, Debug)]
pub struct ScreenshotSurface {
    /// Operator opt-in, read once at startup ([`screenshot_allowed_from_env`]).
    /// Shared by the plain GET, the dynamic POST and `GET /camera`
    /// (agent_vision_20261031 §5.6 — one env var, one field).
    pub allowed: bool,
    /// API→Bevy capture bridge (the plain `GET` path only); `None` on
    /// headless hosts (relay, tests).
    pub bridge: Option<ScreenshotRequestSender>,
    /// Backstop for a capture/camera-read that never answers.
    pub reply_timeout: Duration,
    /// API→Bevy dynamic-capture + camera-read-back bridge (§vision-bridge);
    /// `None` on headless hosts and on every harness that only exercises the
    /// plain GET path. Set via [`Self::with_vision`].
    pub vision: Option<fe_runtime::vision::VisionRequestSender>,
    /// Single-flight guard for posed captures (spec §5.5): a second posed
    /// capture while one is in flight is refused at 409 WITHOUT ever
    /// touching the bridge (`try_acquire_owned`, never blocks).
    pub capture_slot: std::sync::Arc<tokio::sync::Semaphore>,
}

impl ScreenshotSurface {
    /// Off and unwired — the default for every host without a window.
    pub fn disabled() -> Self {
        Self::new(false, None)
    }

    /// A surface with the given opt-in and bridge and the default timeout.
    /// No vision bridge — call [`Self::with_vision`] to wire one.
    pub fn new(allowed: bool, bridge: Option<ScreenshotRequestSender>) -> Self {
        Self {
            allowed,
            bridge,
            reply_timeout: DEFAULT_SCREENSHOT_REPLY_TIMEOUT,
            vision: None,
            capture_slot: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    /// Override the capture timeout (tests drive a short one).
    pub fn with_reply_timeout(mut self, reply_timeout: Duration) -> Self {
        self.reply_timeout = reply_timeout;
        self
    }

    /// Wire the dynamic-capture/camera-read-back bridge (§vision-bridge).
    pub fn with_vision(mut self, vision: fe_runtime::vision::VisionRequestSender) -> Self {
        self.vision = Some(vision);
        self
    }
}

/// Read `FE_API_SCREENSHOT_ALLOW` once at startup (on unless `=0`; garbage
/// fails closed); warns loudly when enabled — see AGENTS.md §screenshot.
pub fn screenshot_allowed_from_env() -> bool {
    SCREENSHOT_GATE.opted_in_from_env()
}

/// Why a capture was refused or failed; each maps to one HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenshotError {
    /// The shared operator gate refused (opt-in, Owner or rate).
    Refused(GateRefusal),
    Unavailable(&'static str),
    TimedOut(Duration),
    CaptureFailed(String),
    /// The token's scope does not cover the active verse (or nothing is
    /// open) — the uniform 404 `NOT_FOUND_OR_DENIED` (§existence-oracle).
    Denied,
}

impl From<GateRefusal> for ScreenshotError {
    fn from(refusal: GateRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl ScreenshotError {
    /// HTTP status for the REST surface.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Refused(refusal) => refusal.status(),
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::CaptureFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Denied => StatusCode::NOT_FOUND,
        }
    }

    /// Client-facing text (REST error body and MCP tool error).
    pub fn message(&self) -> String {
        match self {
            Self::Refused(refusal) => refusal.message(),
            Self::Unavailable(reason) => (*reason).to_string(),
            Self::TimedOut(waited) => format!(
                "screenshot timed out — the window did not answer within {:.1}s",
                waited.as_secs_f32()
            ),
            Self::CaptureFailed(reason) => format!("screenshot failed: {reason}"),
            Self::Denied => crate::rest::NOT_FOUND_OR_DENIED.to_string(),
        }
    }
}

/// Ask the Bevy host for one PNG of the primary window (egui included),
/// carrying the caller's token scope: the host refuses unless it covers the
/// view (SEC-ADDENDUM); the request is audited (DID, scope). Never blocks the
/// async worker: a full bridge is a busy refusal, a silent host a timeout,
/// and a reply echoing a foreign correlation id is rejected.
pub async fn request_capture(
    state: &ApiState,
    claims: &ApiClaims,
) -> Result<Vec<u8>, ScreenshotError> {
    let Some(bridge) = state.screenshot.bridge.as_ref() else {
        return Err(ScreenshotError::Unavailable(
            "screenshot unavailable: this host has no window",
        ));
    };
    crate::vision::audit_capture(claims, None);
    let reply = round_trip(
        bridge,
        &claims.scope,
        CapturePrimaryWindow,
        state.screenshot.reply_timeout,
        "screenshot",
    )
    .await
    .map_err(|error| match error {
        RoundTripError::Saturated => {
            ScreenshotError::Unavailable("screenshot bridge busy — retry shortly")
        }
        RoundTripError::HostGone => {
            ScreenshotError::Unavailable("screenshot unavailable: render host stopped")
        }
        RoundTripError::TimedOut(waited) => ScreenshotError::TimedOut(waited),
        RoundTripError::Uncorrelated => {
            ScreenshotError::CaptureFailed("uncorrelated reply".to_string())
        }
        RoundTripError::Dropped => {
            ScreenshotError::CaptureFailed("capture request dropped".to_string())
        }
    })?;
    reply.map_err(|reason| {
        if reason == fe_runtime::screenshot::SCREENSHOT_SCOPE_DENIED {
            ScreenshotError::Denied
        } else {
            ScreenshotError::CaptureFailed(reason)
        }
    })
}

/// GET /api/v1/debug/screenshot — `image/png` of the live window.
///
/// Gate order (AGENTS.md §screenshot): opt-in → Owner (typed) → 1/s per DID →
/// bridge → host scope check (the token must cover the active verse, else 404).
pub async fn get_screenshot(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> Response {
    match gated_capture(&state, &claims).await {
        Ok(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            png,
        )
            .into_response(),
        Err(error) => (
            error.status(),
            Json(ApiResponse::<()>::error(error.message())),
        )
            .into_response(),
    }
}

/// The REST gate chain end to end: the shared operator gate, then the bridge.
async fn gated_capture(state: &ApiState, claims: &ApiClaims) -> Result<Vec<u8>, ScreenshotError> {
    SCREENSHOT_GATE
        .admit(state.screenshot.allowed, state, claims)
        .await?;
    request_capture(state, claims).await
}
