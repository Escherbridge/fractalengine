//! Fixed native fixture observation/action/capture routes; see AGENTS.md section QA Portal evidence.

use crate::server::ApiState;
use axum::{
    extract::State,
    http::{HeaderValue, StatusCode},
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
    if Instant::now() >= command.deadline {
        return Err(refusal(
            StatusCode::CONFLICT,
            "portal_fixture_timeout",
            "Native fixture request expired",
        ));
    }
    if !state.input.allowed
        || crate::auth::require_role_level(claims, fe_database::RoleLevel::Owner).is_err()
        || state.revoked_jtis.read().await.contains(&claims.jti)
        || std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(true, |now| now.as_secs() >= claims.exp)
    {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "qa_gate_refused",
            "QA authorization is no longer current",
        ));
    }
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
    if crate::auth::require_scope(claims, &result.receipt.scope).is_err()
        || !fe_runtime::qa_web::allows(&result.receipt.url)
        || Instant::now() >= command.deadline
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
) -> Response {
    match run(&state, &claims, FixtureOp::Observe).await {
        Ok(result) => Json(json!({"ok":true,"data":result.receipt})).into_response(),
        Err(error) => error,
    }
}

pub async fn action(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = crate::qa_debug::QA_GATE
        .admit(state.input.allowed, &state, &claims)
        .await
    {
        return refusal(error.status(), "qa_gate_refused", &error.message());
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
) -> Response {
    match run(&state, &claims, FixtureOp::Capture).await {
        Ok(result) => {
            let Some(png) = result.png else {
                return refusal(
                    StatusCode::CONFLICT,
                    "portal_capture_unavailable",
                    "Native PNG unavailable",
                );
            };
            let Some(metadata) = serde_json::to_string(&result.receipt)
                .ok()
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
        }
        Err(error) => error,
    }
}
