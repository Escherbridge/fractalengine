//! Guarded one-use GUI invitation transfer; see AGENTS.md section QA invitation.

use crate::server::ApiState;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::qa_invitation::{
    TransferCommand, TransferMetadata, TransferOperation, TransferReply, JOB_LIMIT,
};
use serde::Serialize;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn refusal(status: StatusCode) -> Response {
    (status,Json(json!({"ok":false,"code":"invitation_transfer_unavailable","error":"The current invitation transfer was not confirmed; do not replay a consumed invitation"}))).into_response()
}
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
async fn admit(state: &ApiState, claims: &ApiClaims) -> Result<(), Response> {
    crate::qa_debug::QA_GATE
        .admit(state.input.allowed, state, claims)
        .await
        .map_err(|error| refusal(error.status()))?;
    crate::auth::require_current_credential(state, claims)
        .await
        .map_err(refusal)
}
async fn run(
    state: &ApiState,
    claims: &ApiClaims,
    operation: TransferOperation,
) -> Result<TransferReply, Response> {
    let bridge = state
        .input
        .invitation_transfer
        .as_ref()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE))?;
    let command = TransferCommand {
        operation,
        issuer_did: claims.sub.clone(),
        deadline: Instant::now() + JOB_LIMIT,
    };
    let result = crate::bridge_client::round_trip(
        bridge,
        &claims.scope,
        command.clone(),
        JOB_LIMIT + Duration::from_secs(1),
        "qa-invitation-transfer",
    )
    .await
    .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE))?;
    if !command.is_current()
        || !state.input.allowed
        || crate::auth::require_role_level(claims, fe_database::RoleLevel::Owner).is_err()
    {
        return Err(refusal(StatusCode::FORBIDDEN));
    }
    crate::auth::require_current_credential(state, claims)
        .await
        .map_err(refusal)?;
    if !command.is_current() {
        return Err(refusal(StatusCode::CONFLICT));
    }
    let reply = result.map_err(|_| refusal(StatusCode::CONFLICT))?;
    let metadata = match &reply {
        TransferReply::Observed {
            local_did,
            invitation,
            ..
        } => {
            if local_did != &claims.sub {
                return Err(refusal(StatusCode::FORBIDDEN));
            }
            invitation.as_ref()
        }
        TransferReply::Consumed { metadata, .. } => Some(metadata),
    };
    if let Some(metadata) = metadata {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE))?
            .as_secs();
        if !metadata.validate()
            || metadata.issuer_did != claims.sub
            || now >= metadata.consume_expires_at
            || crate::auth::require_scope(claims, &metadata.scope).is_err()
        {
            return Err(refusal(StatusCode::FORBIDDEN));
        }
    }
    Ok(reply)
}

pub async fn observe(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    if let Err(response) = admit(&state, &claims).await {
        return no_store(response);
    }
    let response = if uri.query().is_some() {
        refusal(StatusCode::BAD_REQUEST)
    } else {
        match run(&state, &claims, TransferOperation::Observe).await {
            Ok(TransferReply::Observed {
                local_did,
                invitation,
                join_ready_for_input,
            }) => Json(json!({"ok":true,"data":{"local_did":local_did,"invitation":invitation,"join_ready_for_input":join_ready_for_input}}))
                .into_response(),
            Err(response) => response,
            _ => refusal(StatusCode::CONFLICT),
        }
    };
    no_store(response)
}

#[derive(Serialize)]
struct ConsumedData<'a> {
    metadata: TransferMetadata,
    invitation: &'a str,
}
#[derive(Serialize)]
struct ConsumedEnvelope<'a> {
    ok: bool,
    data: ConsumedData<'a>,
}

pub async fn consume(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: Bytes,
) -> Response {
    if let Err(response) = admit(&state, &claims).await {
        return no_store(response);
    }
    let Ok(expected) = serde_json::from_slice::<TransferMetadata>(&body) else {
        return no_store(refusal(StatusCode::BAD_REQUEST));
    };
    if !expected.validate() {
        return no_store(refusal(StatusCode::BAD_REQUEST));
    }
    let response = match run(&state, &claims, TransferOperation::Consume(expected)).await {
        Ok(TransferReply::Consumed {
            metadata,
            credential,
        }) => Json(ConsumedEnvelope {
            ok: true,
            data: ConsumedData {
                metadata,
                invitation: credential.as_str(),
            },
        })
        .into_response(),
        Err(response) => response,
        _ => refusal(StatusCode::CONFLICT),
    };
    no_store(response)
}
