//! Enforced field-schema REST/MCP core; see fe-api/AGENTS.md §property-schema.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use serde_json::{json, Value};

use crate::{server::ApiState, types::ApiResponse};

pub(crate) async fn definition_scope(state: &ApiState, id: &str) -> Option<String> {
    crate::gis::run_select(
        state,
        "SELECT scope FROM field_def WHERE field_def_id = $id LIMIT 1",
        vec![("id".into(), json!(id))],
    )
    .await?
    .first()?
    .get("scope")?
    .as_str()
    .map(str::to_string)
}

pub(crate) async fn get_core(state: &ApiState, id: &str) -> Result<Value, String> {
    let rows = crate::gis::run_select(state,
        "SELECT field_def_id, scope, key, enforced_schema, schema_revisions FROM field_def WHERE field_def_id = $id LIMIT 1",
        vec![("id".into(), json!(id))]).await.ok_or("operation failed")?;
    rows.into_iter()
        .next()
        .ok_or_else(|| crate::rest::NOT_FOUND_OR_DENIED.into())
}

pub(crate) async fn set_core(
    state: &ApiState,
    id: &str,
    descriptor: Value,
) -> Result<Value, String> {
    fe_sdk::property_schema::EnforcedPropertySchema::parse(&descriptor)?;
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::SetFieldSchema {
                field_def_id: id.to_string(),
                descriptor,
                correlation_id: Some(correlation_id.clone()),
            },
            reply_tx,
        })
        .map_err(|_| "internal channel closed")?;
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::QueryResult {
            mut data,
            correlation_id: Some(echoed),
        })) if echoed == correlation_id && data.len() == 1 => Ok(data.remove(0)),
        Ok(Ok(DbResult::RequestFailed {
            error,
            correlation_id: echoed,
        })) if echoed == correlation_id => {
            tracing::warn!(%error, "field schema refused");
            if error.contains("integer exceeds supported signed 64-bit storage range") {
                return Err("integer exceeds supported signed 64-bit storage range".into());
            }
            Err("schema revision refused; existing values, defaults and consecutive version must remain valid".into())
        }
        _ => Err("schema commit was not confirmed; read back before retrying".into()),
    }
}

async fn authorized(
    state: &ApiState,
    claims: &ApiClaims,
    id: &str,
    role: &str,
) -> Result<(), String> {
    crate::auth::require_role(claims, role).map_err(|_| "insufficient permissions")?;
    crate::rest::covered_scope(claims, definition_scope(state, id).await)
        .ok_or(crate::rest::NOT_FOUND_OR_DENIED)?;
    Ok(())
}

pub async fn get(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Value>> {
    let result = match authorized(&state, &claims, &id, "viewer").await {
        Ok(()) => get_core(&state, &id).await,
        Err(error) => Err(error),
    };
    Json(match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::error(error),
    })
}

pub async fn put(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(id): Path<String>,
    Json(descriptor): Json<Value>,
) -> Json<ApiResponse<Value>> {
    let result = match authorized(&state, &claims, &id, "manager").await {
        Ok(()) => set_core(&state, &id, descriptor).await,
        Err(error) => Err(error),
    };
    Json(match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::error(error),
    })
}
