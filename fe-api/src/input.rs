//! `POST /api/v1/debug/input` — synthetic pointer/keyboard/scroll injection
//! for agent-assisted QA and accessibility automation (folded-in
//! `agent_input_surface_20261031`; AIS spec §4-§6, plan I2, `§INPUT-ADDENDUM`).
//! Deny-by-default, Owner-gated, scope-confined — the same `OperatorGate` +
//! correlated-bridge posture as `vision.rs`'s capture door, with one
//! deliberate difference: `INPUT_GATE.default_on == false` (this surface
//! MUTATES state, so it never shares screenshot/camera's default-on
//! posture). See `fe-api/AGENTS.md` §agent-input-surface.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine as _;
use fe_identity::api_token::ApiClaims;
use fe_runtime::input::{
    InputFailure, InputFailureClass, InputOp, InputOutput, InputRefusal, InputRequestSender,
    InputSpec, ValidInputSpec,
};

use crate::bridge_client::{round_trip, RoundTripError};
use crate::operator_gate::{GateRefusal, OperatorGate};
use crate::screenshot::SCREENSHOT_GATE;
use crate::server::ApiState;

// ---------------------------------------------------------------------------
// Gate (reused as DATA — no edits to `operator_gate.rs`'s struct/chain)
// ---------------------------------------------------------------------------

/// Env var for the input surface: unset or anything but `1` is off (fail
/// closed, default-OFF — AIS §6 D-AIS-1, `§INPUT-ADDENDUM` item 3).
pub const INPUT_ALLOW_ENV: &str = "FE_API_INPUT_ALLOW";

/// How long the API waits for the Bevy host to apply a batch, settle for
/// `wait_frames`, and (if requested) capture — mirrors the screenshot/vision
/// surfaces' own `reply_timeout` field.
pub const DEFAULT_INPUT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The input gate: opt-in (OFF unless `=1`; `default_on: false`, the field
/// V2 added to `OperatorGate`) → Owner → 5/s per DID (AIS OD-2: a whole
/// command batch counts once, since a UI interaction is naturally
/// multi-command, unlike a 1/s capture). The host then confines injection to
/// the caller's scope at drain time AND at injection time (check-at-use,
/// `§INPUT-ADDENDUM` item 2) — never an up-front existence check.
pub const INPUT_GATE: OperatorGate = OperatorGate {
    opt_in_env: INPUT_ALLOW_ENV,
    endpoint: "input injection endpoint",
    enabled_warning: "POST /api/v1/debug/input and the MCP `inject_input` tool admit Owner \
                      tokens whose scope covers the open verse to synthesize pointer, keyboard \
                      and scroll input into the running GUI — this MUTATES state, it does not \
                      just observe it. Combined with the default-on screenshot/camera surfaces, \
                      an Owner token now has full remote control of this host's GUI. Set \
                      FE_API_INPUT_ALLOW=1 only on a single-tenant node you are actively driving \
                      with an agent",
    disabled_message: "input injection endpoint is disabled on this node",
    rate_key_prefix: "input",
    rate_per_sec: 5,
    rate_label: "5 input requests/sec",
    // AIS §6 D-AIS-1 / `§INPUT-ADDENDUM` item 3: OFF unless explicitly opted in.
    default_on: false,
};

/// Read `FE_API_INPUT_ALLOW` once at startup (off unless `=1`; any other
/// value fails closed); warns loudly when enabled — AGENTS.md
/// §agent-input-surface.
pub fn input_allowed_from_env() -> bool {
    INPUT_GATE.opted_in_from_env()
}

// ---------------------------------------------------------------------------
// Surface (`ApiState.input`, wired by I3)
// ---------------------------------------------------------------------------

/// The input surface as wired on this host.
#[derive(Clone, Debug)]
pub struct InputSurface {
    /// Operator opt-in, read once at startup ([`input_allowed_from_env`]).
    pub allowed: bool,
    /// API→Bevy input bridge; `None` on headless hosts (relay, every
    /// harness/test `ApiState` that does not exercise this surface) — there
    /// is no window to inject into.
    pub bridge: Option<InputRequestSender>,
    /// Backstop for an injection that never answers.
    pub reply_timeout: Duration,
}

impl InputSurface {
    /// Off and unwired — the default for every host without a window.
    pub fn disabled() -> Self {
        Self::new(false, None)
    }

    /// A surface with the given opt-in and bridge and the default timeout.
    pub fn new(allowed: bool, bridge: Option<InputRequestSender>) -> Self {
        Self {
            allowed,
            bridge,
            reply_timeout: DEFAULT_INPUT_REPLY_TIMEOUT,
        }
    }

    /// Override the reply timeout (tests drive a short one).
    pub fn with_reply_timeout(mut self, reply_timeout: Duration) -> Self {
        self.reply_timeout = reply_timeout;
        self
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why an injection was refused or failed; each maps to one HTTP status (the
/// same shape as `vision::VisionError`).
#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    /// The shared operator gate refused (opt-in, Owner or rate).
    Refused(GateRefusal),
    /// A static validation check failed (`InputSpec::validate`) — 400.
    Refusal(InputRefusal),
    Unavailable(&'static str),
    TimedOut(Duration),
    /// A dynamic (Bevy-side) refusal or failure — includes the uniform scope
    /// denial and "a posed capture holds the camera" (409).
    Failure(InputFailure),
}

impl From<GateRefusal> for InputError {
    fn from(refusal: GateRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl InputError {
    /// HTTP status for the REST surface — follows
    /// [`fe_runtime::input::InputFailureClass`]'s own documented mapping.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Refused(refusal) => refusal.status(),
            Self::Refusal(_) => StatusCode::BAD_REQUEST,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::Failure(failure) => match failure.status_class {
                InputFailureClass::Unresolvable => StatusCode::UNPROCESSABLE_ENTITY,
                InputFailureClass::Busy => StatusCode::CONFLICT,
                InputFailureClass::NoWindow => StatusCode::SERVICE_UNAVAILABLE,
                InputFailureClass::Failed => StatusCode::INTERNAL_SERVER_ERROR,
                InputFailureClass::Denied => StatusCode::NOT_FOUND,
            },
        }
    }

    /// The stable machine code, when this refusal has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refusal(refusal) => Some(refusal.code),
            Self::Failure(failure) => Some(failure.code),
            Self::Refused(_) | Self::Unavailable(_) | Self::TimedOut(_) => None,
        }
    }

    /// The dotted/indexed field path, when this refusal is field-scoped
    /// (`InputRefusal::field` is an owned `String` — a bad command's path
    /// depends on its position in the batch, so it is not `'static`).
    pub fn field(&self) -> Option<String> {
        match self {
            Self::Refusal(refusal) => refusal.field.clone(),
            _ => None,
        }
    }

    /// Client-facing text (REST error body and MCP tool error).
    pub fn message(&self) -> String {
        match self {
            Self::Refused(refusal) => refusal.message(),
            Self::Refusal(refusal) => refusal.message.clone(),
            Self::Unavailable(reason) => (*reason).to_string(),
            Self::TimedOut(waited) => format!(
                "input injection timed out — the window did not answer within {:.1}s",
                waited.as_secs_f32()
            ),
            Self::Failure(failure) => failure.message.clone(),
        }
    }

    /// MCP tool-error text: the message prefixed with its stable `code` (and
    /// `field`), mirroring `vision::VisionError::coded_message` — except the
    /// scope denial, which stays the bare §existence-oracle text on every
    /// door (`fe_runtime::input::DENIED_MESSAGE` is asserted equal to
    /// `fe_runtime::vision::DENIED_MESSAGE` and to `rest::NOT_FOUND_OR_DENIED`
    /// below, so a denied injection reads exactly like a denied capture).
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
}

/// The REST/MCP error body: `{ok:false, error, code?, field?}` — the same
/// shape as `vision::VisionErrorBody`.
#[derive(Debug, serde::Serialize)]
struct InputErrorBody {
    ok: bool,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
}

impl InputErrorBody {
    fn gate(refusal: &GateRefusal) -> Self {
        Self {
            ok: false,
            error: refusal.message(),
            code: None,
            field: None,
        }
    }

    fn refusal(refusal: &InputRefusal) -> Self {
        Self {
            ok: false,
            error: refusal.message.clone(),
            code: Some(refusal.code),
            field: refusal.field.clone(),
        }
    }

    fn error(error: &InputError) -> Self {
        Self {
            ok: false,
            error: error.message(),
            code: error.code(),
            field: error.field(),
        }
    }
}

/// The REST success body: how many commands were applied, how many frames
/// were actually waited, and the optional base64 `capture_after` PNG (REST
/// is JSON-only on this route, unlike the posed-capture route's raw-PNG
/// body + pose header — an injection's primary payload is the outcome, with
/// the image as an attachment, not the other way around).
#[derive(Debug, serde::Serialize)]
struct InputSuccessBody {
    ok: bool,
    commands_applied: u32,
    wait_frames_used: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_after_png_base64: Option<String>,
}

impl From<InputOutput> for InputSuccessBody {
    fn from(output: InputOutput) -> Self {
        Self {
            ok: true,
            commands_applied: output.commands_applied,
            wait_frames_used: output.wait_frames_used,
            capture_after_png_base64: output
                .png
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge round-trip (the shared `bridge_client::round_trip`)
// ---------------------------------------------------------------------------

/// Ask the Bevy host to run one injection on behalf of `caller_scope` (the
/// token's scope — the host refuses unless it covers the active verse at
/// drain time AND at injection time, `§INPUT-ADDENDUM` item 2 — "check at
/// use", never an up-front existence check, so an out-of-scope target and a
/// nonexistent one answer identically). Never blocks the async worker: a
/// full bridge is a busy refusal, a silent host a timeout, and a reply
/// echoing a foreign correlation id is rejected.
pub async fn request_inject(
    state: &ApiState,
    caller_scope: &str,
    spec: ValidInputSpec,
) -> Result<InputOutput, InputError> {
    let Some(bridge) = state.input.bridge.as_ref() else {
        return Err(InputError::Unavailable(
            "input injection unavailable: this host has no window",
        ));
    };
    let reply = round_trip(
        bridge,
        caller_scope,
        InputOp::Inject(spec),
        state.input.reply_timeout,
        "input",
    )
    .await
    .map_err(|error| match error {
        RoundTripError::Saturated => InputError::Unavailable("input bridge busy — retry shortly"),
        RoundTripError::HostGone => {
            InputError::Unavailable("input unavailable: render host stopped")
        }
        RoundTripError::TimedOut(waited) => InputError::TimedOut(waited),
        RoundTripError::Uncorrelated => InputError::Failure(InputFailure::new(
            InputFailureClass::Failed,
            "uncorrelated_reply",
            "uncorrelated reply",
        )),
        RoundTripError::Dropped => InputError::Failure(InputFailure::new(
            InputFailureClass::Failed,
            "request_dropped",
            "input request dropped",
        )),
    })?;
    reply.map_err(InputError::Failure)
}

/// The INFO audit line every admitted injection writes: who acted, how many
/// commands, how long it chose to settle — mirrors `vision::audit_capture`
/// (AGENTS.md §screenshot "Audit"), via [`run_inject`] on both doors.
pub(crate) fn audit_inject(claims: &ApiClaims, spec: &ValidInputSpec) {
    tracing::info!(
        did = %claims.sub,
        scope = %claims.scope,
        commands = spec.commands().len(),
        wait_frames = spec.wait_frames(),
        capture_after = spec.capture_after(),
        "agent input injection requested"
    );
}

/// `capture_after` returns pixels, so it must also clear the screenshot
/// gate's opt-in and per-DID capture budget (`§INPUT-ADDENDUM` item 6) —
/// composed gates, see AGENTS.md §agent-input-surface. The Owner floor is
/// already the input gate's, and the host's view-scope check covers the
/// capture frame (it runs every frame the job lives).
pub(crate) async fn check_capture_after_gate(
    state: &ApiState,
    claims: &ApiClaims,
    spec: &ValidInputSpec,
) -> Result<(), InputError> {
    if !spec.capture_after() {
        return Ok(());
    }
    SCREENSHOT_GATE.check_opted_in(state.screenshot.allowed)?;
    SCREENSHOT_GATE.check_rate(state, claims).await?;
    Ok(())
}

/// Run one injection: the `capture_after` screenshot gate, audit, then cross
/// the bridge. THE one path both doors (REST handler, MCP `inject_input`) share.
pub(crate) async fn run_inject(
    state: &ApiState,
    claims: &ApiClaims,
    spec: ValidInputSpec,
) -> Result<InputOutput, InputError> {
    check_capture_after_gate(state, claims, &spec).await?;
    audit_inject(claims, &spec);
    request_inject(state, &claims.scope, spec).await
}

/// The ONE parse-and-validate path for an injection request, shared by both
/// doors (REST body bytes, MCP arguments): a structural serde failure
/// (including unknown fields — `InputSpec`'s `deny_unknown_fields`) is
/// `malformed_body`, everything else is [`InputSpec::validate`]'s typed
/// table, run in table order (the first failure wins).
pub(crate) fn validate_input_request(
    parsed: Result<InputSpec, serde_json::Error>,
) -> Result<ValidInputSpec, InputRefusal> {
    parsed
        .map_err(|error| InputRefusal {
            code: "malformed_body",
            field: None,
            message: format!("malformed request body: {error}"),
        })?
        .validate()
}

/// `POST`'s raw-body size cap — a batch of up to
/// [`fe_runtime::input::MAX_COMMANDS_PER_REQUEST`] commands, each a handful
/// of floats or a short string, never approaches this; it only bounds the
/// parse itself before the structural `too_many_commands` check runs.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// REST door: a body over [`MAX_BODY_BYTES`] is `malformed_body`. Unlike a
/// capture's bare window shot, there is no sensible default batch, so an
/// empty body resolves through [`InputSpec::validate`] like any other and is
/// refused `no_commands`.
fn parse_and_validate(body: &[u8]) -> Result<ValidInputSpec, InputRefusal> {
    if body.len() > MAX_BODY_BYTES {
        return Err(InputRefusal {
            code: "malformed_body",
            field: None,
            message: format!(
                "request body too large: {} bytes (max {MAX_BODY_BYTES})",
                body.len()
            ),
        });
    }
    if body.is_empty() {
        return InputSpec::default().validate();
    }
    validate_input_request(serde_json::from_slice(body))
}

// ---------------------------------------------------------------------------
// REST handler
// ---------------------------------------------------------------------------

/// `POST /api/v1/debug/input` — synthesize pointer/keyboard/scroll input
/// into the running GUI (AIS spec §4.2, §4.4).
///
/// Gate order (mirrors `vision::post_screenshot`, AIS spec §4.5): opt-in →
/// Owner (typed) → 5/s per DID → static validation (400) → `capture_after`
/// screenshot gate ([`check_capture_after_gate`]) → bridge.
/// Validation runs AFTER the gate on purpose — an unauthenticated or
/// un-opted caller never learns the command grammar, and a refused caller
/// never costs a parse.
pub async fn post_input(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: Bytes,
) -> Response {
    if let Err(refusal) = INPUT_GATE.admit(state.input.allowed, &state, &claims).await {
        return (refusal.status(), Json(InputErrorBody::gate(&refusal))).into_response();
    }
    let spec = match parse_and_validate(&body) {
        Ok(spec) => spec,
        Err(refusal) => {
            return (StatusCode::BAD_REQUEST, Json(InputErrorBody::refusal(&refusal)))
                .into_response();
        }
    };
    match run_inject(&state, &claims, spec).await {
        Ok(output) => (StatusCode::OK, Json(InputSuccessBody::from(output))).into_response(),
        Err(error) => (error.status(), Json(InputErrorBody::error(&error))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One long journey over the whole fe-api input surface (TEST POLICY
    /// 2026-11-01: a brand-new region gets exactly one journey test, not a
    /// scatter of point tests): the gate's own default-off posture, the
    /// parse/validate door shared by REST and MCP, the error→status mapping
    /// table (`fe_runtime::input::InputFailureClass`'s documented contract),
    /// `coded_message`'s prefixing rule and its scope-denial exception, and
    /// the success body's base64 attachment. Each assertion would go red if
    /// its branch inverted.
    #[test]
    fn input_surface_gate_validation_and_error_mapping_journey() {
        // Tripwire (plan.md §7): the input gate never shares the
        // screenshot/camera surfaces' default-on posture. `INPUT_GATE` is a
        // `const`, so clippy treats a direct `assert!` on its field as an
        // assertion on a compile-time constant; routing it through a
        // non-const binding keeps the check a real runtime assertion that
        // would still fail loudly if the const regressed.
        let default_on = std::hint::black_box(INPUT_GATE.default_on);
        assert!(!default_on, "input must stay default-OFF, unlike screenshot/camera");
        assert_eq!(INPUT_GATE.opt_in_env, "FE_API_INPUT_ALLOW");
        assert_eq!(INPUT_GATE.rate_per_sec, 5);

        // The gate's own const resolves the unset/`1`/`0` cases through its
        // `default_on`, exactly like every other gate's env parsing.
        assert!(!crate::operator_gate::opt_in_value_enables(None, INPUT_GATE.default_on));
        assert!(crate::operator_gate::opt_in_value_enables(Some("1"), INPUT_GATE.default_on));
        assert!(!crate::operator_gate::opt_in_value_enables(Some("0"), INPUT_GATE.default_on));

        // `InputSurface::disabled()` is off and unwired, the default for
        // every host without a window.
        let disabled = InputSurface::disabled();
        assert!(!disabled.allowed);
        assert!(disabled.bridge.is_none());

        // The shared parse-and-validate door: malformed JSON, then an empty
        // body (no sensible default batch, unlike a capture), then a
        // well-formed minimal request.
        let malformed = parse_and_validate(b"{not json")
            .expect_err("malformed JSON is refused before validate() ever runs");
        assert_eq!(malformed.code, "malformed_body");

        let empty = parse_and_validate(b"").expect_err("an empty body has no commands");
        assert_eq!(empty.code, "no_commands");

        let too_big = parse_and_validate(&vec![b' '; MAX_BODY_BYTES + 1])
            .expect_err("an oversized body is refused before it is even parsed");
        assert_eq!(too_big.code, "malformed_body");

        let valid_body = serde_json::json!({
            "commands": [{ "kind": "pointer_click", "x": 1.0, "y": 2.0, "button": "left" }],
        })
        .to_string();
        let valid = parse_and_validate(valid_body.as_bytes())
            .expect("a well-formed single click round-trips through the REST door");
        assert_eq!(valid.commands().len(), 1);

        // Error -> HTTP status mapping, following
        // `fe_runtime::input::InputFailureClass`'s own documented contract.
        let status_cases = [
            (
                InputError::Refused(GateRefusal::Disabled("off")),
                StatusCode::FORBIDDEN,
            ),
            (
                InputError::Refused(GateRefusal::InsufficientRole),
                StatusCode::FORBIDDEN,
            ),
            (
                InputError::Refused(GateRefusal::RateLimited("slow down".to_string())),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                InputError::Refusal(InputRefusal {
                    code: "non_finite",
                    field: Some("commands[0].x".to_string()),
                    message: "bad".to_string(),
                }),
                StatusCode::BAD_REQUEST,
            ),
            (InputError::Unavailable("no window"), StatusCode::SERVICE_UNAVAILABLE),
            (InputError::TimedOut(Duration::from_secs(5)), StatusCode::GATEWAY_TIMEOUT),
            (
                InputError::Failure(InputFailure {
                    status_class: InputFailureClass::Unresolvable,
                    code: "x",
                    message: "x".into(),
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                InputError::Failure(fe_runtime::input::camera_locked_by_capture()),
                StatusCode::CONFLICT,
            ),
            (
                InputError::Failure(InputFailure {
                    status_class: InputFailureClass::NoWindow,
                    code: "no_window",
                    message: "x".into(),
                }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                InputError::Failure(InputFailure {
                    status_class: InputFailureClass::Failed,
                    code: "x",
                    message: "x".into(),
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                InputError::Failure(fe_runtime::input::denied()),
                StatusCode::NOT_FOUND,
            ),
        ];
        for (error, expected) in status_cases {
            assert_eq!(error.status(), expected, "{error:?}");
        }

        // `coded_message` prefixes code(+field), except the bare scope
        // denial — which is byte-identical to the REST existence-oracle text
        // and to vision's own denial, so an agent sees one uniform string
        // whether a capture or an injection was denied.
        assert_eq!(fe_runtime::input::DENIED_MESSAGE, crate::rest::NOT_FOUND_OR_DENIED);
        assert_eq!(fe_runtime::input::DENIED_MESSAGE, fe_runtime::vision::DENIED_MESSAGE);
        let field_scoped = InputError::Refusal(InputRefusal {
            code: "non_finite",
            field: Some("commands[0].x".to_string()),
            message: "must be finite".to_string(),
        });
        assert_eq!(field_scoped.coded_message(), "non_finite (commands[0].x): must be finite");
        let unscoped = InputError::Failure(fe_runtime::input::camera_locked_by_capture());
        assert_eq!(
            unscoped.coded_message(),
            format!("{}: {}", fe_runtime::input::CAMERA_LOCKED_BY_CAPTURE_CODE, unscoped.message())
        );
        let denial = InputError::Failure(fe_runtime::input::denied());
        assert_eq!(denial.coded_message(), crate::rest::NOT_FOUND_OR_DENIED);

        // The success body carries the base64 attachment only when a PNG
        // came back, and omits the field entirely otherwise.
        let with_png = InputSuccessBody::from(InputOutput {
            commands_applied: 1,
            wait_frames_used: 10,
            png: Some(vec![1, 2, 3]),
        });
        let encoded = serde_json::to_value(&with_png).expect("serializes");
        assert_eq!(
            encoded["capture_after_png_base64"],
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode([1, 2, 3]))
        );

        let without_png = InputSuccessBody::from(InputOutput {
            commands_applied: 2,
            wait_frames_used: 0,
            png: None,
        });
        let encoded_without = serde_json::to_value(&without_png).expect("serializes");
        assert!(
            encoded_without.get("capture_after_png_base64").is_none(),
            "no PNG means the field is omitted, not null"
        );
    }
}
