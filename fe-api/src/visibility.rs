//! Petal visibility-Group registry endpoints (hierarchy_visibility_groups
//! Phase 6): REST `GET/PUT /api/v1/petals/{p}/visibility-groups`,
//! `PATCH/DELETE …/visibility-groups/{g}`, and the shared cores behind the MCP
//! `list_visibility_groups` / `set_visibility_group` tools. Auth model,
//! read-modify-write window: fe-api AGENTS.md §visibility-groups.

use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use fe_database::RoleLevel;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, CallerAuth, DbCommand, DbResult, VisibilityGroupDto};
use serde::Deserialize;

use crate::auth::require_role_level;
use crate::endpoint::caller_auth;
use crate::types::{is_valid_ulid, ApiResponse};

/// How long a registry read/write waits for the DB thread's correlated echo.
const VISIBILITY_TIMEOUT_SECS: u64 = 10;

/// Why a registry request did not land; never carries DB internals.
#[derive(Debug, PartialEq)]
pub(crate) enum VisibilityError {
    /// The registry (or patch) fails validation.
    Invalid(String),
    /// PATCH without a `name` for a new group / DELETE of an unknown group.
    GroupNotFound,
    /// The DB thread refused (fe-policy denial) or failed.
    Failed,
    /// No reply within [`VISIBILITY_TIMEOUT_SECS`].
    TimedOut,
}

impl VisibilityError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::GroupNotFound => StatusCode::NOT_FOUND,
            Self::Failed => StatusCode::BAD_GATEWAY,
            Self::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::Invalid(detail) => format!("invalid visibility groups: {detail}"),
            Self::GroupNotFound => "group not found".to_string(),
            Self::Failed => "visibility groups request failed or not permitted".to_string(),
            Self::TimedOut => "visibility groups request timed out (db thread)".to_string(),
        }
    }
}

impl IntoResponse for VisibilityError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(ApiResponse::<serde_json::Value>::error(self.message())),
        )
            .into_response()
    }
}

/// One correlated DB round trip expecting `PetalVisibilityGroupsLoaded` for
/// `petal_id` (the id AND petal must match — defense in depth over the
/// router's correlated delivery, like `terrain::set_petal_terrain_core`).
async fn registry_round_trip(
    state: &crate::server::ApiState,
    petal_id: &str,
    build: impl FnOnce(String) -> DbCommand,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = build(correlation_id.clone());
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .is_err()
    {
        tracing::warn!(petal_id, "visibility groups: API command channel closed");
        return Err(VisibilityError::Failed);
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(VISIBILITY_TIMEOUT_SECS),
        reply_rx,
    )
    .await
    {
        Ok(Ok(DbResult::PetalVisibilityGroupsLoaded {
            petal_id: reply_petal,
            groups,
            correlation_id: Some(reply_id),
        })) if reply_id == correlation_id && reply_petal == petal_id => Ok(groups),
        Ok(Ok(DbResult::RequestFailed {
            error,
            correlation_id: reply_id,
        })) if reply_id == correlation_id => {
            tracing::warn!(petal_id, error = %error, "visibility groups refused/failed on the DB thread");
            Err(VisibilityError::Failed)
        }
        Ok(Ok(other)) => {
            tracing::warn!(
                petal_id,
                ?other,
                "visibility groups: reply mismatch — refused"
            );
            Err(VisibilityError::Failed)
        }
        Ok(Err(_)) => Err(VisibilityError::Failed),
        Err(_) => Err(VisibilityError::TimedOut),
    }
}

/// Read the registry (caller has authorized Viewer+ at the petal).
pub(crate) async fn get_groups_core(
    state: &crate::server::ApiState,
    petal_id: &str,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    registry_round_trip(state, petal_id, |correlation_id| {
        DbCommand::GetPetalVisibilityGroups {
            petal_id: petal_id.to_string(),
            correlation_id: Some(correlation_id),
        }
    })
    .await
}

/// Replace the whole registry (caller has authorized; the DB thread re-checks
/// Editor+ at the petal scope via fe-policy).
pub(crate) async fn set_groups_core(
    state: &crate::server::ApiState,
    petal_id: &str,
    groups: Vec<VisibilityGroupDto>,
    auth: CallerAuth,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    fe_database::handlers::visibility_groups::validate_groups(&groups)
        .map_err(VisibilityError::Invalid)?;
    registry_round_trip(state, petal_id, |correlation_id| {
        DbCommand::SetPetalVisibilityGroups {
            petal_id: petal_id.to_string(),
            groups,
            auth,
            correlation_id: Some(correlation_id),
        }
    })
    .await
}

/// A partial update of one Group (`PATCH` body / MCP `set_visibility_group`).
#[derive(Debug, Default, Deserialize)]
pub struct GroupPatch {
    pub name: Option<String>,
    pub visible: Option<bool>,
    pub order: Option<f64>,
}

/// Pure upsert: patch `group_id` in place, or append it (a NEW group needs a
/// `name`; `visible` defaults true, `order` to after the current last).
pub(crate) fn upsert_group(
    mut groups: Vec<VisibilityGroupDto>,
    group_id: &str,
    patch: GroupPatch,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    if let Some(group) = groups.iter_mut().find(|g| g.id == group_id) {
        if let Some(name) = patch.name {
            group.name = name;
        }
        if let Some(visible) = patch.visible {
            group.visible = visible;
        }
        if let Some(order) = patch.order {
            group.order = order;
        }
        return Ok(groups);
    }
    let Some(name) = patch.name else {
        return Err(VisibilityError::GroupNotFound);
    };
    let next_order = groups.iter().map(|g| g.order).fold(-1.0_f64, f64::max) + 1.0;
    groups.push(VisibilityGroupDto {
        id: group_id.to_string(),
        name,
        visible: patch.visible.unwrap_or(true),
        order: patch.order.unwrap_or(next_order),
    });
    Ok(groups)
}

/// Pure removal of `group_id`; unknown id → `GroupNotFound`.
pub(crate) fn remove_group(
    mut groups: Vec<VisibilityGroupDto>,
    group_id: &str,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    let before = groups.len();
    groups.retain(|g| g.id != group_id);
    if groups.len() == before {
        return Err(VisibilityError::GroupNotFound);
    }
    Ok(groups)
}

/// Read-modify-write of one Group under a correlated Get→Set pair. Last
/// writer wins between the two (a concurrent registry write in that window is
/// overwritten) — documented in AGENTS.md §visibility-groups.
pub(crate) async fn modify_group_core(
    state: &crate::server::ApiState,
    petal_id: &str,
    auth: CallerAuth,
    modify: impl FnOnce(Vec<VisibilityGroupDto>) -> Result<Vec<VisibilityGroupDto>, VisibilityError>,
) -> Result<Vec<VisibilityGroupDto>, VisibilityError> {
    let current = get_groups_core(state, petal_id).await?;
    set_groups_core(state, petal_id, modify(current)?, auth).await
}

/// Shared REST gate: role floor → valid petal id → token covers the petal
/// (missing and foreign are the SAME `NOT_FOUND_OR_DENIED`).
async fn gate(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    petal_id: &str,
    role: RoleLevel,
) -> Result<(), Response> {
    if require_role_level(claims, role).is_err() {
        return Err(Json(ApiResponse::<serde_json::Value>::error(
            "insufficient permissions",
        ))
        .into_response());
    }
    if !is_valid_ulid(petal_id) {
        return Err(
            Json(ApiResponse::<serde_json::Value>::error("invalid petal_id")).into_response(),
        );
    }
    if crate::rest::authorized_petal_scope(state, claims, petal_id)
        .await
        .is_none()
    {
        return Err(Json(ApiResponse::<serde_json::Value>::error(
            crate::rest::NOT_FOUND_OR_DENIED,
        ))
        .into_response());
    }
    Ok(())
}

/// REST envelope: `{petal_id, groups}` or the error status.
fn registry_response(
    petal_id: &str,
    result: Result<Vec<VisibilityGroupDto>, VisibilityError>,
) -> Response {
    match result {
        Ok(groups) => Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "groups": groups,
        })))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT` body: the whole registry.
#[derive(Debug, Deserialize)]
pub struct ReplaceGroupsBody {
    pub groups: Vec<VisibilityGroupDto>,
}

/// GET /api/v1/petals/:petal_id/visibility-groups — Viewer+.
pub async fn list_visibility_groups(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if let Err(denied) = gate(&state, &claims, &petal_id, RoleLevel::Viewer).await {
        return denied;
    }
    registry_response(&petal_id, get_groups_core(&state, &petal_id).await)
}

/// PUT /api/v1/petals/:petal_id/visibility-groups — Editor+, whole-registry replace.
pub async fn replace_visibility_groups(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(body): Json<ReplaceGroupsBody>,
) -> Response {
    if let Err(denied) = gate(&state, &claims, &petal_id, RoleLevel::Editor).await {
        return denied;
    }
    registry_response(
        &petal_id,
        set_groups_core(&state, &petal_id, body.groups, caller_auth(&claims)).await,
    )
}

/// PATCH /api/v1/petals/:petal_id/visibility-groups/:group_id — Editor+, upsert one.
pub async fn patch_visibility_group(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((petal_id, group_id)): Path<(String, String)>,
    Json(patch): Json<GroupPatch>,
) -> Response {
    if let Err(denied) = gate(&state, &claims, &petal_id, RoleLevel::Editor).await {
        return denied;
    }
    registry_response(
        &petal_id,
        modify_group_core(&state, &petal_id, caller_auth(&claims), |groups| {
            upsert_group(groups, &group_id, patch)
        })
        .await,
    )
}

/// DELETE /api/v1/petals/:petal_id/visibility-groups/:group_id — Editor+.
pub async fn delete_visibility_group(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((petal_id, group_id)): Path<(String, String)>,
) -> Response {
    if let Err(denied) = gate(&state, &claims, &petal_id, RoleLevel::Editor).await {
        return denied;
    }
    registry_response(
        &petal_id,
        modify_group_core(&state, &petal_id, caller_auth(&claims), |groups| {
            remove_group(groups, &group_id)
        })
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, order: f64) -> VisibilityGroupDto {
        VisibilityGroupDto {
            id: id.into(),
            name: id.to_uppercase(),
            visible: true,
            order,
        }
    }

    #[test]
    fn upsert_patches_existing_and_appends_new_after_the_last() {
        let groups = vec![group("a", 0.0), group("b", 4.0)];
        let patched = upsert_group(
            groups.clone(),
            "a",
            GroupPatch {
                visible: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!patched[0].visible);
        assert_eq!(patched[0].name, "A", "unpatched fields stay");

        let appended = upsert_group(
            groups.clone(),
            "c",
            GroupPatch {
                name: Some("New".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(appended.len(), 3);
        assert_eq!((appended[2].visible, appended[2].order), (true, 5.0));

        assert_eq!(
            upsert_group(groups, "c", GroupPatch::default()),
            Err(VisibilityError::GroupNotFound),
            "a new group needs a name"
        );
    }

    #[test]
    fn remove_drops_one_group_or_reports_unknown() {
        let groups = vec![group("a", 0.0), group("b", 1.0)];
        let ids: Vec<String> = remove_group(groups.clone(), "a")
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(ids, vec!["b".to_string()]);
        assert_eq!(
            remove_group(groups, "zzz"),
            Err(VisibilityError::GroupNotFound)
        );
    }
}
