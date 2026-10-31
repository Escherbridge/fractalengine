//! `GET /api/v1/debug/screenshot` + the MCP `screenshot` tool's shared core:
//! deny-by-default agent visual verification — see AGENTS.md §screenshot.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::screenshot::{ScreenshotRequest, ScreenshotRequestSender};

use crate::server::ApiState;
use crate::types::ApiResponse;

/// Env var that opts a node into the screenshot surface (only `1` enables).
pub const SCREENSHOT_ALLOW_ENV: &str = "FE_API_SCREENSHOT_ALLOW";

/// How long the API waits for the Bevy host to capture and encode a frame.
pub const DEFAULT_SCREENSHOT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-DID capture budget (a screenshot can show any verse's data).
pub const SCREENSHOT_RATE_PER_SEC: u32 = 1;

/// The refusal every call gets while the operator has not opted in.
pub const SCREENSHOT_DISABLED: &str = "screenshot endpoint is disabled on this node";

/// The screenshot surface as wired on this host (`ApiState.screenshot`).
#[derive(Clone, Debug)]
pub struct ScreenshotSurface {
    /// Operator opt-in, read once at startup ([`screenshot_allowed_from_env`]).
    pub allowed: bool,
    /// API→Bevy capture bridge; `None` on headless hosts (relay, tests).
    pub bridge: Option<ScreenshotRequestSender>,
    /// Backstop for a capture that never answers.
    pub reply_timeout: Duration,
}

impl ScreenshotSurface {
    /// Off and unwired — the default for every host without a window.
    pub fn disabled() -> Self {
        Self::new(false, None)
    }

    /// A surface with the given opt-in and bridge and the default timeout.
    pub fn new(allowed: bool, bridge: Option<ScreenshotRequestSender>) -> Self {
        Self {
            allowed,
            bridge,
            reply_timeout: DEFAULT_SCREENSHOT_REPLY_TIMEOUT,
        }
    }

    /// Override the capture timeout (tests drive a short one).
    pub fn with_reply_timeout(mut self, reply_timeout: Duration) -> Self {
        self.reply_timeout = reply_timeout;
        self
    }
}

/// Read the `FE_API_SCREENSHOT_ALLOW` opt-in once at startup (only `1` enables);
/// warns loudly when enabled — see AGENTS.md §screenshot.
pub fn screenshot_allowed_from_env() -> bool {
    match std::env::var(SCREENSHOT_ALLOW_ENV) {
        Ok(v) if v == "1" => {
            tracing::warn!(
                "{SCREENSHOT_ALLOW_ENV}=1: GET /api/v1/debug/screenshot and the MCP `screenshot` \
                 tool are ENABLED for Owner tokens. A capture shows the whole window — whatever \
                 verse is open, regardless of the token's scope. Enable only on a single-tenant \
                 node you are actively driving with an agent"
            );
            true
        }
        Ok(v) if !v.is_empty() => {
            tracing::warn!(
                "{SCREENSHOT_ALLOW_ENV}={v:?} ignored (only `1` enables) — screenshot endpoint \
                 stays disabled"
            );
            false
        }
        _ => false,
    }
}

/// Why a capture was refused or failed; each maps to one HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenshotError {
    Disabled,
    InsufficientRole,
    RateLimited(String),
    Unavailable(&'static str),
    TimedOut(Duration),
    CaptureFailed(String),
}

impl ScreenshotError {
    /// HTTP status for the REST surface.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Disabled | Self::InsufficientRole => StatusCode::FORBIDDEN,
            Self::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::CaptureFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Client-facing text (REST error body and MCP tool error).
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => SCREENSHOT_DISABLED.to_string(),
            Self::InsufficientRole => "insufficient permissions (owner required)".to_string(),
            Self::RateLimited(reason) => reason.clone(),
            Self::Unavailable(reason) => (*reason).to_string(),
            Self::TimedOut(waited) => format!(
                "screenshot timed out — the window did not answer within {:.1}s",
                waited.as_secs_f32()
            ),
            Self::CaptureFailed(reason) => format!("screenshot failed: {reason}"),
        }
    }
}

/// Gate 1: the operator opt-in, checked before role, rate limit or bridge.
pub fn ensure_enabled(state: &ApiState) -> Result<(), ScreenshotError> {
    if state.screenshot.allowed {
        Ok(())
    } else {
        Err(ScreenshotError::Disabled)
    }
}

/// Gate 3: one capture per second per DID (shares the query limiter map).
pub async fn ensure_rate(state: &ApiState, claims: &ApiClaims) -> Result<(), ScreenshotError> {
    crate::query_guard::check_rate_limit(
        state,
        &format!("screenshot:{}", claims.sub),
        SCREENSHOT_RATE_PER_SEC,
        "1 screenshot/sec",
    )
    .await
    .map_err(ScreenshotError::RateLimited)
}

/// Ask the Bevy host for one PNG of the primary window (egui included). Never
/// blocks the async worker: a full bridge is a busy refusal, a silent host a
/// timeout, and a reply echoing a foreign correlation id is rejected.
pub async fn request_capture(state: &ApiState) -> Result<Vec<u8>, ScreenshotError> {
    let Some(bridge) = state.screenshot.bridge.as_ref() else {
        return Err(ScreenshotError::Unavailable(
            "screenshot unavailable: this host has no window",
        ));
    };
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let request = ScreenshotRequest {
        correlation_id: correlation_id.clone(),
        reply_tx,
    };
    match bridge.try_send(request) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            return Err(ScreenshotError::Unavailable(
                "screenshot bridge busy — retry shortly",
            ))
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            tracing::warn!("screenshot bridge closed (Bevy host gone)");
            return Err(ScreenshotError::Unavailable(
                "screenshot unavailable: render host stopped",
            ));
        }
    }
    match tokio::time::timeout(state.screenshot.reply_timeout, reply_rx).await {
        Ok(Ok(reply)) if reply.correlation_id == correlation_id => {
            reply.result.map_err(ScreenshotError::CaptureFailed)
        }
        Ok(Ok(_)) => Err(ScreenshotError::CaptureFailed(
            "uncorrelated reply".to_string(),
        )),
        Ok(Err(_)) => Err(ScreenshotError::CaptureFailed(
            "capture request dropped".to_string(),
        )),
        Err(_) => Err(ScreenshotError::TimedOut(state.screenshot.reply_timeout)),
    }
}

/// GET /api/v1/debug/screenshot — `image/png` of the live window.
///
/// Gate order (AGENTS.md §screenshot): opt-in → Owner (typed) → 1/s per DID →
/// bridge. NOT scope-confined: the frame shows whatever verse is open.
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

/// The REST gate chain end to end.
async fn gated_capture(state: &ApiState, claims: &ApiClaims) -> Result<Vec<u8>, ScreenshotError> {
    ensure_enabled(state)?;
    if crate::auth::require_role_level(claims, fe_database::RoleLevel::Owner).is_err() {
        return Err(ScreenshotError::InsufficientRole);
    }
    ensure_rate(state, claims).await?;
    request_capture(state).await
}
