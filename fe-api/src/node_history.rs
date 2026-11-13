//! Scoped ordinary node history and committed mutations; see AGENTS.md §node-history.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{DbCommand, DbResult, NodeMutation};
use fe_runtime::scene_hook_gate::SceneCallerKind;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp::scene_gate::{admit, scope_petal_id, Door, WriteFailure};
use crate::{server::ApiState, types::ApiResponse};

/// Bounded forward cursor page; versions are per object, not global time.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    #[serde(default)]
    pub after_version: i64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// Exact committed version to reconstruct.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateQuery {
    pub at_version: i64,
}

pub(crate) async fn resolved_scope(state: &ApiState, node_id: &str) -> Option<String> {
    if let Some(scope) = crate::rest::resolve_node_scope(state, node_id).await {
        return Some(scope);
    }
    crate::gis::run_select(state,
        "SELECT payload.scope AS scope, row_version FROM node_log WHERE node_id = $id AND payload.journal_version = 1 ORDER BY row_version DESC LIMIT 1",
        vec![("id".into(), json!(node_id))]).await?
        .first()?.get("scope")?.as_str().map(str::to_string)
}

pub(crate) async fn read_core(
    state: &ApiState,
    claims: &ApiClaims,
    node_id: &str,
    after_version: i64,
    limit: usize,
    at_version: Option<i64>,
) -> Result<Value, String> {
    if after_version < 0 || !(1..=200).contains(&limit) || at_version.is_some_and(|v| v < 1) {
        return Err("history requires after_version >= 0, limit 1..200 and at_version >= 1".into());
    }
    let id = ulid::Ulid::new().to_string();
    let value = correlated(
        state,
        DbCommand::ReadNodeHistory {
            node_id: node_id.to_string(),
            after_version,
            limit,
            at_version,
            grant: fe_runtime::node_history::NodeAccessGrant::Scoped(claims.scope.clone()),
            correlation_id: Some(id.clone()),
        },
        &id,
    )
    .await?;
    crate::query_guard::enforce_byte_ceiling(
        std::slice::from_ref(&value),
        crate::limits::QUERY_MAX_RESPONSE_BYTES,
        crate::limits::QUERY_MAX_RESPONSE_LABEL,
    )?;
    Ok(value)
}

/// Commit one mutation through the scene write door (hooks, augments), then the DB round trip.
pub(crate) async fn commit_core(
    state: &ApiState,
    door: Door<'_>,
    petal_id: &str,
    node_id: &str,
    mutation: NodeMutation,
) -> Result<Value, WriteFailure> {
    let id = ulid::Ulid::new().to_string();
    let command = DbCommand::CommitNodeMutation {
        node_id: node_id.to_string(),
        mutation,
        actor_did: door.claims.sub.clone(),
        grant: fe_runtime::node_history::NodeAccessGrant::Scoped(door.claims.scope.clone()),
        correlation_id: Some(id.clone()),
    };
    let (command, admission) = admit(door, petal_id, command).await?;
    let committed = correlated(state, command, &id).await?;
    admission.stage(None);
    Ok(committed)
}

/// Create one object through the scene write door (create + initial property hooks), then the DB round trip.
pub(crate) async fn create_core(
    state: &ApiState,
    door: Door<'_>,
    petal_id: &str,
    input: fe_runtime::messages::CreateObjectInput,
) -> Result<Value, WriteFailure> {
    if let Some(asset_id) = &input.asset_id {
        crate::upload::validate_scene_asset(state, door.claims, asset_id).await?;
    }
    let id = ulid::Ulid::new().to_string();
    let command = DbCommand::CreateObject {
        petal_id: petal_id.to_string(),
        input,
        actor_did: door.claims.sub.clone(),
        grant: fe_runtime::node_history::NodeAccessGrant::Scoped(door.claims.scope.clone()),
        correlation_id: Some(id.clone()),
    };
    let (command, admission) = admit(door, petal_id, command).await?;
    let created = correlated(state, command, &id).await?;
    let created_node_id = created
        .get("node_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    admission.stage(created_node_id.as_deref());
    Ok(created)
}

#[derive(PartialEq)]
enum HistoryReplyFamily {
    Committed,
    Page,
    State,
}

enum HistoryTarget {
    Node(String),
    Petal(String),
}

impl HistoryTarget {
    fn matches_node(&self, node_id: &str) -> bool {
        matches!(self,Self::Node(expected) if expected == node_id)
    }
    fn matches_receipt(&self, receipt: &fe_runtime::node_history::NodeMutationReceipt) -> bool {
        receipt.node_id == receipt.state.node_id
            && match self {
                Self::Node(node_id) => node_id == &receipt.node_id,
                Self::Petal(petal_id) => {
                    petal_id == &receipt.state.petal_id
                        && crate::types::is_valid_ulid(&receipt.node_id)
                }
            }
    }
}

async fn correlated(state: &ApiState, command: DbCommand, id: &str) -> Result<Value, String> {
    let target = match &command {
        DbCommand::ReadNodeHistory { node_id, .. }
        | DbCommand::CommitNodeMutation { node_id, .. } => HistoryTarget::Node(node_id.clone()),
        DbCommand::CreateObject { petal_id, .. } => HistoryTarget::Petal(petal_id.clone()),
        _ => return Err("unsupported history command".into()),
    };
    let requested_version = match &command {
        DbCommand::ReadNodeHistory { at_version, .. } => *at_version,
        _ => None,
    };
    let expected = match &command {
        DbCommand::ReadNodeHistory {
            at_version: Some(_),
            ..
        } => HistoryReplyFamily::State,
        DbCommand::ReadNodeHistory { .. } => HistoryReplyFamily::Page,
        DbCommand::CreateObject { .. } | DbCommand::CommitNodeMutation { .. } => {
            HistoryReplyFamily::Committed
        }
        _ => return Err("unsupported history command".into()),
    };
    match crate::rest::db_round_trip(state, command).await? {
        DbResult::NodeMutationCommitted {
            receipt,
            correlation_id: Some(echoed),
        } if echoed == id
            && expected == HistoryReplyFamily::Committed
            && target.matches_receipt(&receipt) =>
        {
            serde_json::to_value(receipt).map_err(|_| "invalid committed receipt".into())
        }
        DbResult::NodeHistoryLoaded {
            page,
            correlation_id: Some(echoed),
        } if echoed == id
            && expected == HistoryReplyFamily::Page
            && target.matches_node(&page.coverage.node_id)
            && page
                .entries
                .iter()
                .all(|entry| target.matches_node(&entry.node_id)) =>
        {
            serde_json::to_value(page).map_err(|_| "invalid history page".into())
        }
        DbResult::NodeStateAtVersionLoaded {
            state,
            correlation_id: Some(echoed),
        } if echoed == id
            && expected == HistoryReplyFamily::State
            && target.matches_node(&state.coverage.node_id)
            && requested_version == Some(state.at_version)
            && state
                .state
                .as_ref()
                .is_none_or(|snapshot| target.matches_node(&snapshot.node_id)) =>
        {
            serde_json::to_value(state).map_err(|_| "invalid historical state".into())
        }
        DbResult::RequestFailed {
            error,
            correlation_id,
        } if correlation_id == id => {
            tracing::warn!(%error, "node history operation refused");
            if error.contains("integer exceeds supported signed 64-bit storage range") {
                return Err("integer exceeds supported signed 64-bit storage range".into());
            }
            Err(
                "operation refused; validate the value/schema and requested history coverage"
                    .into(),
            )
        }
        _ => Err("committed reply not confirmed; read back before retrying".into()),
    }
}

async fn authorized(
    state: &ApiState,
    claims: &ApiClaims,
    node_id: &str,
    role: &str,
) -> Result<String, String> {
    crate::auth::require_role(claims, role).map_err(|_| "insufficient permissions")?;
    crate::rest::covered_scope(claims, resolved_scope(state, node_id).await)
        .ok_or_else(|| crate::rest::NOT_FOUND_OR_DENIED.to_string())
}

fn response(result: Result<Value, String>) -> Json<ApiResponse<Value>> {
    Json(match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::error(error),
    })
}

pub async fn history(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<ApiResponse<Value>> {
    response(
        match authorized(&state, &claims, &node_id, "viewer").await {
            Ok(_) => {
                read_core(
                    &state,
                    &claims,
                    &node_id,
                    query.after_version,
                    query.limit,
                    None,
                )
                .await
            }
            Err(error) => Err(error),
        },
    )
}

pub async fn state_at(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Query(query): Query<StateQuery>,
) -> Json<ApiResponse<Value>> {
    response(
        match authorized(&state, &claims, &node_id, "viewer").await {
            Ok(_) => read_core(&state, &claims, &node_id, 0, 1, Some(query.at_version)).await,
            Err(error) => Err(error),
        },
    )
}

pub async fn mutate(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Json(mutation): Json<NodeMutation>,
) -> Json<ApiResponse<Value>> {
    response(
        match authorized(&state, &claims, &node_id, "editor").await {
            Ok(scope) => match scope_petal_id(&scope) {
                Some(petal_id) => {
                    let door = Door { claims: &claims, kind: SceneCallerKind::Rest };
                    commit_core(&state, door, &petal_id, &node_id, mutation)
                        .await
                        .map_err(WriteFailure::message)
                }
                None => Err("could not resolve node petal".to_string()),
            },
            Err(error) => Err(error),
        },
    )
}

pub async fn create(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(input): Json<fe_runtime::messages::CreateObjectInput>,
) -> Json<ApiResponse<Value>> {
    let result = if crate::auth::require_role(&claims, "editor").is_err() {
        Err("insufficient permissions".into())
    } else if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        Err(crate::rest::NOT_FOUND_OR_DENIED.into())
    } else {
        let door = Door { claims: &claims, kind: SceneCallerKind::Rest };
        create_core(&state, door, &petal_id, input)
            .await
            .map_err(WriteFailure::message)
    };
    response(result)
}
