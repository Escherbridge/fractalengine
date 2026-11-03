//! Node-bound read-only QA asset audit; see ../AGENTS.md scene-asset-publication-audit.

use crate::{qa_debug::QA_GATE, server::ApiState};
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::{bridge::BridgeRequest, qa_debug::QaOp, scene_asset::*};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

type Failure = (StatusCode, &'static str);
fn query(uri: &Uri) -> Result<Option<String>, Failure> {
    if uri.query().is_some_and(|s| s.len() > 128) {
        return Err((StatusCode::BAD_REQUEST, "invalid_scene_asset_audit_query"));
    }
    let Query(values) = Query::<Vec<(String, String)>>::try_from_uri(uri)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_scene_asset_audit_query"))?;
    if values.len() > 1 {
        return Err((StatusCode::BAD_REQUEST, "invalid_scene_asset_audit_query"));
    }
    match values.first() {
        None => Ok(None),
        Some((key, value)) if key == "operation_id" && is_canonical_audit_ulid(value) => {
            Ok(Some(value.clone()))
        }
        _ => Err((StatusCode::BAD_REQUEST, "invalid_scene_asset_audit_query")),
    }
}

async fn read(
    state: &ApiState,
    claims: &ApiClaims,
    node_id: &str,
    uri: &Uri,
) -> Result<SceneAssetPublicationAudit, Failure> {
    QA_GATE
        .admit(state.input.allowed, state, claims)
        .await
        .map_err(|e| (e.status(), "qa_gate_refused"))?;
    crate::auth::require_current_credential(state, claims)
        .await
        .map_err(|s| (s, "credential_refused"))?;
    if !fe_runtime::safe_id::is_safe_path_id(node_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid_node_id"));
    }
    let operation_id = query(uri)?;
    let bridge = state
        .input
        .qa_debug
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "qa_unavailable"))?;
    let deadline = Instant::now() + SCENE_ASSET_AUDIT_LIMIT;
    let nonce = ulid::Ulid::new().to_string();
    let input = SceneAssetAuditRequest {
        node_id: node_id.into(),
        operation_id: operation_id.clone(),
        caller: SceneAssetAuditCaller {
            did: claims.sub.clone(),
            scope: claims.scope.clone(),
            role_ceiling: claims.max_role.clone(),
        },
        deadline,
    };
    let (request, reply) = BridgeRequest::new(
        nonce.clone(),
        claims.scope.clone(),
        QaOp::SceneAssetPublication(input),
    );
    bridge
        .try_send(request)
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "scene_asset_audit_busy"))?;
    let reply = tokio::time::timeout(SCENE_ASSET_AUDIT_LIMIT + Duration::from_secs(1), reply)
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "scene_asset_audit_timeout"))?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "scene_asset_audit_unavailable",
            )
        })?;
    if reply.correlation_id != nonce || Instant::now() >= deadline {
        return Err((StatusCode::CONFLICT, "scene_asset_audit_uncorrelated"));
    }
    let value = reply
        .result
        .map_err(|_| (StatusCode::CONFLICT, "scene_asset_audit_refused"))?;
    if serde_json::to_vec(&value)
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "scene_asset_audit_unavailable",
            )
        })?
        .len()
        > SCENE_ASSET_AUDIT_RESPONSE_MAX
    {
        return Err((StatusCode::CONFLICT, "scene_asset_audit_response_limit"));
    }
    let output: SceneAssetPublicationAudit = serde_json::from_value(value)
        .map_err(|_| (StatusCode::CONFLICT, "scene_asset_audit_uncorrelated"))?;
    if output.request_nonce != nonce
        || output.protocol_version != 1
        || output.node_id != node_id
        || output
            .adoption_operation
            .as_ref()
            .map(|op| op.operation_id.as_str())
            != operation_id.as_deref()
    {
        return Err((StatusCode::CONFLICT, "scene_asset_audit_uncorrelated"));
    }
    crate::auth::require_scope(claims, &output.scope).map_err(|s| (s, "scope_refused"))?;
    crate::auth::require_current_credential(state, claims)
        .await
        .map_err(|s| (s, "credential_refused"))?;
    if Instant::now() >= deadline {
        return Err((StatusCode::CONFLICT, "scene_asset_audit_expired"));
    }
    Ok(output)
}

pub async fn get(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    uri: Uri,
    _body: axum::body::Bytes,
) -> Response {
    let mut response = match read(&state, &claims, &node_id, &uri).await {
        Ok(data) => Json(json!({"ok":true,"data":data})).into_response(),
        Err((status, code)) => (
            status,
            Json(
                json!({"ok":false,"code":code,"error":"Scene asset audit refused or unavailable"}),
            ),
        )
            .into_response(),
    };
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
