//! Fixed native fixture observation/action/capture routes; see AGENTS.md section QA Portal evidence.

use crate::server::ApiState;
use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::qa_portal::{FixtureCommand, FixtureOp, FixtureResult, JOB_LIMIT};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    operation: FixtureOp,
}

async fn run(
    state: &ApiState,
    claims: &ApiClaims,
    operation: FixtureOp,
) -> Result<FixtureResult, Response> {
    crate::qa_debug::QA_GATE
        .admit(state.input.allowed, state, claims)
        .await
        .map_err(|error| {
            (
                error.status(),
                Json(json!({"ok":false,"code":"qa_gate_refused","error":error.message()})),
            )
                .into_response()
        })?;
    admitted(state, claims, operation).await
}

async fn admitted(
    state: &ApiState,
    claims: &ApiClaims,
    operation: FixtureOp,
) -> Result<FixtureResult, Response> {
    let bridge = state.input.portal_fixture.as_ref().ok_or_else(|| {
        refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "portal_fixture_unavailable",
            "Native fixture bridge unavailable",
        )
    })?;
    let command = FixtureCommand {
        operation,
        deadline: Instant::now() + JOB_LIMIT,
    };
    let result = crate::bridge_client::round_trip(
        bridge,
        &claims.scope,
        command.clone(),
        JOB_LIMIT + Duration::from_secs(1),
        "qa-portal-fixture",
    )
    .await
    .map_err(|_| {
        refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "portal_fixture_timeout",
            "Native fixture bridge unavailable or timed out",
        )
    })?;
    // Authentication and lifetime may have changed while native work was pending.
    if !command.is_current() {
        return Err(refusal(
            StatusCode::CONFLICT,
            "portal_fixture_timeout",
            "Native fixture request expired",
        ));
    }
    if !state.input.allowed
        || crate::auth::require_role_level(claims, fe_database::RoleLevel::Owner).is_err()
    {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "qa_gate_refused",
            "QA authorization is no longer current",
        ));
    }
    crate::auth::require_current_credential(state, claims)
        .await
        .map_err(|status| {
            refusal(
                status,
                "qa_gate_refused",
                "QA authorization is no longer current",
            )
        })?;
    let result = result.map_err(|error| {
        refusal(
            if error.code == "not_found_or_denied" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            },
            error.code,
            error.message,
        )
    })?;
    let url_ok = if matches!(operation, FixtureOp::PortalKillSwitch { .. }) {
        // Host governance op: no page, so the receipt carries no URL.
        result.receipt.url.is_empty()
    } else if matches!(operation, FixtureOp::PortalIpcStats) && unmounted_totals(&result) {
        result.receipt.url.is_empty()
    } else if is_bundle_operation(operation) {
        is_bundle_origin_url(&result.receipt.url)
    } else {
        fe_runtime::qa_web::allows(&result.receipt.url)
    };
    if crate::auth::require_scope(claims, &result.receipt.scope).is_err()
        || !url_ok
        || !command.is_current()
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "portal_fixture_changed",
            "Native fixture scope or permit is no longer current",
        ));
    }
    Ok(result)
}

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"ok":false,"code":code,"error":message})),
    )
        .into_response()
}

pub async fn observe(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    if uri.query().is_some() {
        return reject_arguments(&state, &claims).await;
    }
    match run(&state, &claims, FixtureOp::Observe).await {
        Ok(result) => Json(json!({"ok":true,"data":result.receipt})).into_response(),
        Err(error) => error,
    }
}

pub async fn action(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = crate::qa_debug::QA_GATE
        .admit(state.input.allowed, &state, &claims)
        .await
    {
        return refusal(error.status(), "qa_gate_refused", &error.message());
    }
    if uri.query().is_some() {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_fixture_operation",
            "Fixture operations accept no query arguments",
        );
    }
    let action: Action = match serde_json::from_slice(&body) {
        Ok(action) => action,
        Err(_) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "invalid_fixture_operation",
                "Only a fixed fixture operation is accepted",
            )
        }
    };
    if !matches!(
        action.operation,
        FixtureOp::ActivateSecondLink | FixtureOp::ActivateRefusedThirdLink
    ) {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_fixture_operation",
            "Only fixed fixture link operations are accepted",
        );
    }
    match admitted(&state, &claims, action.operation).await {
        Ok(result) => Json(json!({"ok":true,"data":result.receipt})).into_response(),
        Err(error) => error,
    }
}

pub async fn capture(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    if uri.query().is_some() || !body.is_empty() {
        return reject_arguments(&state, &claims).await;
    }
    match run(&state, &claims, FixtureOp::Capture).await {
        Ok(result) => {
            let Some(png) = result.png else {
                return refusal(
                    StatusCode::CONFLICT,
                    "portal_capture_unavailable",
                    "Native PNG unavailable",
                );
            };
            if png.len() > fe_runtime::qa_portal::MAX_PNG_BYTES {
                return refusal(
                    StatusCode::CONFLICT,
                    "portal_capture_invalid",
                    "Native PNG exceeds its byte limit",
                );
            }
            let Some(metadata) = serde_json::to_string(&result.receipt)
                .ok()
                .filter(|text| text.len() <= 4096)
                .and_then(|s| HeaderValue::from_str(&s).ok())
            else {
                return refusal(
                    StatusCode::CONFLICT,
                    "invalid_capture_receipt",
                    "Native capture receipt unavailable",
                );
            };
            let mut response = png.into_response();
            response
                .headers_mut()
                .insert("content-type", HeaderValue::from_static("image/png"));
            response
                .headers_mut()
                .insert("x-fe-portal-fixture", metadata);
            response
                .headers_mut()
                .insert("cache-control", HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => error,
    }
}

async fn reject_arguments(state: &ApiState, claims: &ApiClaims) -> Response {
    match crate::qa_debug::QA_GATE
        .admit(state.input.allowed, state, claims)
        .await
    {
        Err(error) => refusal(error.status(), "qa_gate_refused", &error.message()),
        Ok(()) => refusal(
            StatusCode::BAD_REQUEST,
            "invalid_fixture_operation",
            "This fixed fixture endpoint accepts no additional arguments",
        ),
    }
}

/// The bundle operations: fixed observe, numbered slot and IPC counters (protocol.md section 9).
fn is_bundle_operation(operation: FixtureOp) -> bool {
    matches!(
        operation,
        FixtureOp::PortalObserve | FixtureOp::PortalSlot(_) | FixtureOp::PortalIpcStats
    )
}

/// True when ipc-stats answered process totals with no mounted bundle (R-POR-31).
fn unmounted_totals(result: &FixtureResult) -> bool {
    result
        .receipt
        .portal
        .as_ref()
        .is_some_and(|portal| portal.get("mounted") == Some(&serde_json::Value::Bool(false)))
}

/// True for `https://fe-portal.<label>.localhost[/...]` (or the non-Windows `fe-portal://` spelling) with a 28-char label.
fn is_bundle_origin_url(raw: &str) -> bool {
    let Some(rest) = raw
        .strip_prefix("https://fe-portal.")
        .or_else(|| raw.strip_prefix("fe-portal://"))
    else {
        return false;
    };
    let Some((label, tail)) = rest.split_once(".localhost") else {
        return false;
    };
    let label_ok = label.len() == 28
        && matches!(label.as_bytes()[0], b'a' | b's')
        && label.as_bytes()[1] == b'-'
        && label.as_bytes()[2..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(c));
    label_ok && (tail.is_empty() || tail.starts_with('/'))
}

fn bundle_reply(result: FixtureResult) -> Response {
    let Some(portal) = result.receipt.portal else {
        return refusal(
            StatusCode::CONFLICT,
            "portal_fixture_document_mismatch",
            "Native bundle evidence unavailable",
        );
    };
    Json(json!({"ok":true,"data":{
        "url": result.receipt.url,
        "popup_session": result.receipt.popup_session,
        "page_generation": result.receipt.page_generation,
        "portal": portal,
    }}))
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotRequest {
    slot: u8,
}

/// GET /api/v1/debug/portal/observe: title, probe digest and named probe texts of the mounted bundle.
pub async fn portal_observe(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    if uri.query().is_some() {
        return reject_arguments(&state, &claims).await;
    }
    match run(&state, &claims, FixtureOp::PortalObserve).await {
        Ok(result) => bundle_reply(result),
        Err(error) => error,
    }
}

/// GET /api/v1/debug/portal/ipc-stats: the host's IPC outcome counters.
pub async fn portal_ipc_stats(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    if uri.query().is_some() {
        return reject_arguments(&state, &claims).await;
    }
    match run(&state, &claims, FixtureOp::PortalIpcStats).await {
        Ok(result) => bundle_reply(result),
        Err(error) => error,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillSwitchRequest {
    engage: bool,
}

/// POST /api/v1/debug/portal/killswitch: `{"engage": bool}` only; gated like `portal_action`.
/// The binary's bridge engages or restores the portal resolver's host-disabled state and echoes the outcome.
pub async fn portal_killswitch(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = crate::qa_debug::QA_GATE
        .admit(state.input.allowed, &state, &claims)
        .await
    {
        return refusal(error.status(), "qa_gate_refused", &error.message());
    }
    let request = match serde_json::from_slice::<KillSwitchRequest>(&body) {
        Ok(request) if uri.query().is_none() => request,
        _ => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "invalid_fixture_operation",
                "Only {\"engage\": true|false} is accepted",
            )
        }
    };
    if crate::auth::require_role_level(&claims, fe_database::RoleLevel::Owner).is_err() {
        return refusal(
            StatusCode::FORBIDDEN,
            "qa_gate_refused",
            "QA authorization is no longer current",
        );
    }
    match admitted(&state, &claims, FixtureOp::PortalKillSwitch { engage: request.engage }).await {
        Ok(result) => Json(json!({"ok":true,"data":{
            "scope": result.receipt.scope,
            "killswitch": result.receipt.portal.and_then(|portal| portal.get("killswitch").cloned()),
        }}))
        .into_response(),
        Err(error) => error,
    }
}

/// POST /api/v1/debug/portal/action: activate `data-fe-slot` number 1..32; the only input is the number.
pub async fn portal_action(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = crate::qa_debug::QA_GATE
        .admit(state.input.allowed, &state, &claims)
        .await
    {
        return refusal(error.status(), "qa_gate_refused", &error.message());
    }
    let request = match serde_json::from_slice::<SlotRequest>(&body) {
        Ok(request) if uri.query().is_none() && (1..=32).contains(&request.slot) => request,
        _ => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "invalid_fixture_operation",
                "Only a slot number 1..32 is accepted",
            )
        }
    };
    match admitted(&state, &claims, FixtureOp::PortalSlot(request.slot)).await {
        Ok(result) => bundle_reply(result),
        Err(error) => error,
    }
}
