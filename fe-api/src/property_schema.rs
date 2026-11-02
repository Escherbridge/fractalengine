//! Enforced field-schema REST/MCP core; see fe-api/AGENTS.md §property-schema.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{DbCommand, DbResult};
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

pub(crate) async fn get_core(
    state: &ApiState,
    claims: &ApiClaims,
    id: &str,
) -> Result<Value, String> {
    schema_round_trip(state, id, |correlation_id| DbCommand::GetFieldSchema {
        field_def_id: id.to_string(),
        grant: fe_runtime::node_history::NodeAccessGrant::Scoped(claims.scope.clone()),
        correlation_id: Some(correlation_id),
    })
    .await
}

pub(crate) async fn set_core(
    state: &ApiState,
    claims: &ApiClaims,
    id: &str,
    descriptor: Value,
) -> Result<Value, String> {
    fe_sdk::property_schema::EnforcedPropertySchema::parse(&descriptor)?;
    schema_round_trip(state, id, |correlation_id| DbCommand::SetFieldSchema {
        field_def_id: id.to_string(),
        descriptor,
        grant: fe_runtime::node_history::NodeAccessGrant::Scoped(claims.scope.clone()),
        correlation_id: Some(correlation_id),
    })
    .await
}

async fn schema_round_trip(
    state: &ApiState,
    id: &str,
    command: impl FnOnce(String) -> DbCommand,
) -> Result<Value, String> {
    let correlation_id = ulid::Ulid::new().to_string();
    match crate::rest::db_round_trip(state, command(correlation_id.clone())).await? {
        DbResult::QueryResult {
            mut data,
            correlation_id: Some(echoed),
        } if echoed == correlation_id
            && data.len() == 1
            && data[0]["field_def_id"].as_str() == Some(id) =>
        {
            Ok(data.remove(0))
        }
        DbResult::RequestFailed {
            error,
            correlation_id: echoed,
        } if echoed == correlation_id => {
            tracing::warn!(%error,"field schema refused");
            if error.contains("integer exceeds supported signed 64-bit storage range") {
                return Err("integer exceeds supported signed 64-bit storage range".into());
            }
            Err("schema revision refused; check permissions, stored values/defaults and consecutive version".into())
        }
        _ => Err("schema reply not confirmed; read back before retrying".into()),
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
        Ok(()) => get_core(&state, &claims, &id).await,
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
        Ok(()) => set_core(&state, &claims, &id, descriptor).await,
        Err(error) => Err(error),
    };
    Json(match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::error(error),
    })
}
