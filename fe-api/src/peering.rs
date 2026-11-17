//! Permanent REST + MCP lifecycle surfaces for node peering identity,
//! addressed P2P invitations, joining verses, and role management.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_database::join_invite::{JoinInvitation, JoinManifest, JoinRole};
use fe_identity::api_token::ApiClaims;
use fe_runtime::entity_access::{EntityAccessAction, EntityAccessTarget, EntityGrantRecord};
use fe_runtime::join_invite::JoinCredential;
use fe_runtime::messages::{DbCommand, DbResult, EntityType};
use fe_runtime::peering::{NodePeeringIdentity, PeeringCall, PeeringCallSender};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{require_role_and_scope, require_role_level, require_scope};
use crate::network_ops::NetworkOps;
use crate::rest::{covered_scope, db_round_trip, resolve_verse_scope};
use crate::server::{ApiHosts, ApiState};
use crate::types::is_valid_ulid;

const PEERING_CALL_TIMEOUT: Duration = Duration::from_secs(10);
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub(crate) struct PeeringError {
    pub status: StatusCode,
    pub message: String,
}

impl PeeringError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "ok": false, "data": null, "error": self.message })),
        )
            .into_response()
    }
}

fn peering_sender<'a>(
    hosts: Option<&'a ApiHosts>,
    ops: Option<&'a NetworkOps>,
) -> Option<&'a PeeringCallSender> {
    hosts
        .and_then(|h| h.peering.as_ref())
        .or_else(|| ops.and_then(|o| o.peering.as_ref()))
}

fn local_node_did(state: &ApiState) -> String {
    fe_identity::pub_key_to_did_key(&state.verifying_key)
}

fn require_local_operator(state: &ApiState, claims: &ApiClaims) -> Result<String, PeeringError> {
    let did = local_node_did(state);
    if claims.sub != did {
        return Err(PeeringError::new(
            StatusCode::FORBIDDEN,
            "local node operator token required",
        ));
    }
    Ok(did)
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn target_for_scope(verse_id: &str, scope: Option<&str>) -> Result<EntityAccessTarget, PeeringError> {
    if !is_valid_ulid(verse_id) {
        return Err(PeeringError::new(StatusCode::BAD_REQUEST, "invalid verse_id"));
    }
    let scope_str = match scope.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None => format!("VERSE#{verse_id}"),
    };
    let parts = fe_database::parse_scope(&scope_str)
        .map_err(|_| PeeringError::new(StatusCode::BAD_REQUEST, "invalid scope"))?;
    if parts.verse_id != verse_id {
        return Err(PeeringError::new(
            StatusCode::BAD_REQUEST,
            "scope does not belong to verse_id",
        ));
    }
    let (entity_type, entity_id) = match (parts.fractal_id.as_ref(), parts.petal_id.as_ref()) {
        (_, Some(petal_id)) => (EntityType::Petal, petal_id.clone()),
        (Some(fractal_id), None) => (EntityType::Fractal, fractal_id.clone()),
        (None, None) => (EntityType::Verse, parts.verse_id),
    };
    Ok(EntityAccessTarget {
        entity_type,
        entity_id,
        scope: scope_str,
    })
}

async fn require_covered_verse_target(
    state: &ApiState,
    claims: &ApiClaims,
    verse_id: &str,
    scope: Option<&str>,
    min_role: &str,
) -> Result<EntityAccessTarget, PeeringError> {
    let target = target_for_scope(verse_id, scope)?;
    let resolved = resolve_verse_scope(state, verse_id).await;
    if covered_scope(claims, resolved).is_none() || require_scope(claims, &target.scope).is_err() {
        return Err(PeeringError::new(
            StatusCode::NOT_FOUND,
            crate::rest::NOT_FOUND_OR_DENIED,
        ));
    }
    if require_role_and_scope(claims, min_role, &target.scope).is_err() {
        return Err(PeeringError::new(
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        ));
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Identity core + REST
// ---------------------------------------------------------------------------

pub(crate) async fn get_identity_core(
    _state: &ApiState,
    claims: &ApiClaims,
    sender: Option<&PeeringCallSender>,
) -> Result<NodePeeringIdentity, PeeringError> {
    let Some(sender) = sender else {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering service is unavailable",
        ));
    };
    let detail = require_role_level(claims, fe_database::RoleLevel::Owner).is_ok()
        && crate::network::detail_enabled_from_env();
    let (reply, rx) = tokio::sync::oneshot::channel();
    if sender.try_send(PeeringCall::GetIdentity { detail, reply }).is_err() {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering bridge unavailable",
        ));
    }
    match tokio::time::timeout(PEERING_CALL_TIMEOUT, rx).await {
        Ok(Ok(mut id)) => {
            if require_role_level(claims, fe_database::RoleLevel::Owner).is_err() {
                id.direct_addrs.clear();
                id.home_relay = None;
            }
            Ok(id)
        }
        Ok(Err(_)) => Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering identity unavailable",
        )),
        Err(_) => Err(PeeringError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "peering identity timed out",
        )),
    }
}

pub async fn get_identity(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    hosts: Option<Extension<ApiHosts>>,
    ops: Option<Extension<NetworkOps>>,
) -> Response {
    if require_role_level(&claims, fe_database::RoleLevel::Viewer).is_err() {
        return PeeringError::new(StatusCode::FORBIDDEN, "insufficient permissions").into_response();
    }
    let sender = peering_sender(
        hosts.as_ref().map(|Extension(h)| h),
        ops.as_ref().map(|Extension(o)| o),
    );
    match get_identity_core(&state, &claims, sender).await {
        Ok(id) => Json(json!({ "ok": true, "data": id })).into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Issue verse invite core + REST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct IssueInviteRequest {
    pub recipient_did: String,
    pub role: Option<String>,
    pub expiry_hours: Option<u32>,
    pub scope: Option<String>,
}

pub(crate) async fn issue_verse_invite_core(
    state: &ApiState,
    claims: &ApiClaims,
    sender: Option<&PeeringCallSender>,
    verse_id: &str,
    req: IssueInviteRequest,
) -> Result<Value, PeeringError> {
    let target = require_covered_verse_target(
        state,
        claims,
        verse_id,
        req.scope.as_deref(),
        "manager",
    )
    .await?;
    let local_did = require_local_operator(state, claims)?;
    let recipient_did = req.recipient_did.trim().to_owned();
    if !recipient_did.starts_with("did:key:") || recipient_did.len() > 256 || recipient_did == local_did {
        return Err(PeeringError::new(
            StatusCode::BAD_REQUEST,
            "invalid recipient_did",
        ));
    }
    let role = req.role.unwrap_or_else(|| "editor".to_owned());
    let parsed_role = JoinRole::parse(&role)
        .map_err(|_| PeeringError::new(StatusCode::BAD_REQUEST, "unsupported invite role"))?;
    let expiry_hours = req.expiry_hours.unwrap_or(24);
    if expiry_hours == 0 || expiry_hours > 720 {
        return Err(PeeringError::new(
            StatusCode::BAD_REQUEST,
            "expiry_hours must be between 1 and 720",
        ));
    }
    let Some(sender) = sender else {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering service is unavailable",
        ));
    };

    let generation = ulid::Ulid::new().to_string();
    let prepare_reply = db_round_trip(
        state,
        DbCommand::PrepareJoinInvite {
            target: target.clone(),
            recipient_did: recipient_did.clone(),
            role: role.clone(),
            expiry_hours,
            request_id: format!("{generation}-prepare"),
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    let source = match prepare_reply {
        DbResult::JoinInvitePrepared {
            result: Ok(source), ..
        } => source,
        DbResult::JoinInvitePrepared { result: Err(e), .. } => {
            return Err(PeeringError::new(StatusCode::BAD_REQUEST, e));
        }
        DbResult::Error(e) => return Err(PeeringError::new(StatusCode::BAD_GATEWAY, e)),
        _ => {
            return Err(PeeringError::new(
                StatusCode::BAD_GATEWAY,
                "invite preparation failed: unexpected db reply",
            ))
        }
    };

    let manifest: JoinManifest = serde_json::from_value(source.manifest.clone()).map_err(|e| {
        PeeringError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid invite manifest: {e}"),
        )
    })?;

    let (ticket_tx, ticket_rx) = tokio::sync::oneshot::channel();
    if sender
        .try_send(PeeringCall::ExportJoinTicket {
            request_id: generation.clone(),
            verse_id: manifest.verse_id.clone(),
            namespace_id: manifest.namespace_id.clone(),
            issuer_did: local_did.clone(),
            role: parsed_role.as_str().to_owned(),
            deadline_secs: 15,
            reply: ticket_tx,
        })
        .is_err()
    {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering bridge unavailable",
        ));
    }

    let ticket_str = match tokio::time::timeout(Duration::from_secs(16), ticket_rx).await {
        Ok(Ok(Ok(ticket))) => ticket,
        Ok(Ok(Err(e))) => return Err(PeeringError::new(StatusCode::CONFLICT, e)),
        Ok(Err(_)) => {
            return Err(PeeringError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ticket export dropped",
            ))
        }
        Err(_) => {
            return Err(PeeringError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "ticket export timed out",
            ))
        }
    };

    let ticket = JoinCredential::new(ticket_str)
        .map_err(|e| PeeringError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let issue_reply = db_round_trip(
        state,
        DbCommand::IssueJoinInvite {
            source,
            ticket,
            request_id: format!("{generation}-issue"),
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    let issued = match issue_reply {
        DbResult::JoinInviteIssued {
            result: Ok(issued), ..
        } => issued,
        DbResult::JoinInviteIssued { result: Err(e), .. } => {
            return Err(PeeringError::new(StatusCode::BAD_REQUEST, e));
        }
        DbResult::Error(e) => return Err(PeeringError::new(StatusCode::BAD_GATEWAY, e)),
        _ => {
            return Err(PeeringError::new(
                StatusCode::BAD_GATEWAY,
                "invite issue failed: unexpected db reply",
            ))
        }
    };

    Ok(json!({
        "invite": issued.credential.as_str(),
        "verse_id": verse_id,
        "scope": issued.source.target.scope,
        "role": issued.source.role,
        "recipient_did": issued.source.recipient_did,
        "expires_at": issued.expires_at,
        "issuer_did": local_did,
    }))
}

pub async fn issue_verse_invite(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    hosts: Option<Extension<ApiHosts>>,
    ops: Option<Extension<NetworkOps>>,
    Path(verse_id): Path<String>,
    Json(req): Json<IssueInviteRequest>,
) -> Response {
    let sender = peering_sender(
        hosts.as_ref().map(|Extension(h)| h),
        ops.as_ref().map(|Extension(o)| o),
    );
    match issue_verse_invite_core(&state, &claims, sender, &verse_id, req).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Join verse core + REST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JoinVerseRequest {
    pub invite: String,
    pub wait: Option<bool>,
    pub timeout_secs: Option<u64>,
}

pub(crate) async fn join_verse_core(
    state: &ApiState,
    claims: &ApiClaims,
    sender: Option<&PeeringCallSender>,
    req: JoinVerseRequest,
) -> Result<Value, PeeringError> {
    if require_role_level(claims, fe_database::RoleLevel::Owner).is_err() {
        return Err(PeeringError::new(
            StatusCode::FORBIDDEN,
            crate::operator_gate::OWNER_REQUIRED,
        ));
    }
    let local_did = require_local_operator(state, claims)?;
    let Some(sender) = sender else {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering service is unavailable",
        ));
    };

    let cred = JoinCredential::new(req.invite.trim().to_owned())
        .map_err(|e| PeeringError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let now = unix_now_secs();
    let invitation = JoinInvitation::parse(cred.as_str(), &local_did, now)
        .map_err(|e| PeeringError::new(StatusCode::BAD_REQUEST, format!("invalid invite: {e}")))?;

    let timeout_secs = req.timeout_secs.unwrap_or(20).clamp(1, 60);
    let generation = format!("join-{}", ulid::Ulid::new());

    let admit_reply = db_round_trip(
        state,
        DbCommand::AdmitJoinInvite {
            credential: cred.clone(),
            request_id: generation.clone(),
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    let admission = match admit_reply {
        DbResult::JoinInviteAdmitted {
            result: Ok(admitted),
            ..
        } => admitted,
        DbResult::JoinInviteAdmitted { result: Err(e), .. } => {
            return Err(PeeringError::new(StatusCode::BAD_REQUEST, e));
        }
        DbResult::Error(e) => return Err(PeeringError::new(StatusCode::BAD_GATEWAY, e)),
        _ => {
            return Err(PeeringError::new(
                StatusCode::BAD_GATEWAY,
                "join admission failed: unexpected db reply",
            ))
        }
    };

    let (open_tx, open_rx) = tokio::sync::oneshot::channel();
    if sender
        .try_send(PeeringCall::JoinVerseReplica {
            request_id: generation.clone(),
            credential: cred.as_str().to_owned(),
            deadline_secs: timeout_secs,
            reply: open_tx,
        })
        .is_err()
    {
        return Err(PeeringError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "peering bridge unavailable",
        ));
    }

    match tokio::time::timeout(Duration::from_secs(timeout_secs + 2), open_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => return Err(PeeringError::new(StatusCode::CONFLICT, e)),
        Ok(Err(_)) => {
            return Err(PeeringError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "replica open dropped",
            ))
        }
        Err(_) => {
            return Err(PeeringError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "replica open timed out",
            ))
        }
    }

    if !req.wait.unwrap_or(true) {
        return Ok(json!({
            "status": "replica_opened",
            "converged": false,
            "verse_id": admission.verse_id,
            "verse_name": admission.verse_name,
            "scope": admission.scope,
            "role": admission.role,
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut round = 0u32;
    let mut transport_ready = false;
    let mut role_ready = false;

    while Instant::now() < deadline {
        round = round.saturating_add(1);
        if !transport_ready {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if sender
                .try_send(PeeringCall::InspectJoinedReplica {
                    request_id: generation.clone(),
                    credential: cred.as_str().to_owned(),
                    deadline_secs: timeout_secs,
                    reply: tx,
                })
                .is_ok()
            {
                if let Ok(Ok(Ok(ready))) =
                    tokio::time::timeout(Duration::from_secs(5), rx).await
                {
                    transport_ready = ready;
                }
            }
        }
        if !role_ready {
            let check = db_round_trip(
                state,
                DbCommand::CheckJoinReadiness {
                    credential: cred.clone(),
                    request_id: format!("{generation}-role-{round}"),
                },
            )
            .await;
            if let Ok(DbResult::JoinReadinessChecked {
                result: Ok(true), ..
            }) = check
            {
                role_ready = true;
            }
        }
        if transport_ready && role_ready {
            break;
        }
        tokio::time::sleep(JOIN_POLL_INTERVAL).await;
    }

    if !(transport_ready && role_ready) {
        return Err(PeeringError::new(
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "admitted locally but timed out waiting for P2P convergence (transport_ready={transport_ready}, role_ready={role_ready})"
            ),
        ));
    }

    let requires_proof = invitation
        .recipient_manager_proof(unix_now_secs())
        .map_err(|e| PeeringError::new(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_some();

    if requires_proof {
        let finalize = db_round_trip(
            state,
            DbCommand::FinalizeJoinProof {
                credential: cred,
                request_id: format!("{generation}-finalize"),
                deadline: Instant::now() + Duration::from_secs(5),
            },
        )
        .await
        .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;
        match finalize {
            DbResult::JoinProofFinalized { result: Ok(()), .. } => {}
            DbResult::JoinProofFinalized { result: Err(e), .. } => {
                return Err(PeeringError::new(StatusCode::CONFLICT, e));
            }
            _ => {
                return Err(PeeringError::new(
                    StatusCode::BAD_GATEWAY,
                    "join proof finalization failed: unexpected db reply",
                ))
            }
        }
    }

    Ok(json!({
        "status": "ready",
        "converged": true,
        "verse_id": admission.verse_id,
        "verse_name": admission.verse_name,
        "scope": admission.scope,
        "role": admission.role,
    }))
}

pub async fn join_verse(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    hosts: Option<Extension<ApiHosts>>,
    ops: Option<Extension<NetworkOps>>,
    Json(req): Json<JoinVerseRequest>,
) -> Response {
    let sender = peering_sender(
        hosts.as_ref().map(|Extension(h)| h),
        ops.as_ref().map(|Extension(o)| o),
    );
    match join_verse_core(&state, &claims, sender, req).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Roles management core + REST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ScopeQuery {
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub peer_did: String,
    pub role: String,
    pub scope: Option<String>,
}

fn grants_to_json(grants: &[EntityGrantRecord]) -> Vec<Value> {
    grants
        .iter()
        .map(|g| {
            json!({
                "peer_did": g.peer_did,
                "grantor_did": g.grantor_did,
                "role": g.role,
                "revoked": g.revoked,
                "hlc": g.hlc,
            })
        })
        .collect()
}

pub(crate) async fn list_roles_core(
    state: &ApiState,
    claims: &ApiClaims,
    verse_id: &str,
    scope: Option<&str>,
) -> Result<Value, PeeringError> {
    let target = require_covered_verse_target(state, claims, verse_id, scope, "viewer").await?;

    if let Some(db) = &state.db_reader {
        let mut scopes = vec![target.scope.clone()];
        while let Some(parent) = fe_database::parent_scope(scopes.last().unwrap()) {
            scopes.push(parent);
        }
        let res = db
            .query(
                "SELECT peer_did, scope, role FROM role WHERE scope IN $scopes ORDER BY scope, peer_did LIMIT 512;\
                 SELECT statement_key, scope, peer_did, grantor_did, role, revoked, hlc, updated_at FROM role_grant WHERE scope = $scope ORDER BY hlc DESC LIMIT 256;\
                 SELECT peer_did, status FROM verse_member WHERE verse_id = $verse_id ORDER BY peer_did LIMIT 256;",
            )
            .bind(("scopes", scopes))
            .bind(("scope", target.scope.clone()))
            .bind(("verse_id", verse_id.to_owned()))
            .await;
        if let Ok(mut resp) = res.and_then(|r| r.check().map_err(Into::into)) {
            let role_rows: Vec<Value> = resp.take(0).unwrap_or_default();
            let grants: Vec<Value> = resp.take(1).unwrap_or_default();
            let members: Vec<Value> = resp.take(2).unwrap_or_default();
            return Ok(json!({
                "verse_id": verse_id,
                "scope": target.scope,
                "local_did": local_node_did(state),
                "roles": role_rows,
                "grants": grants,
                "members": members,
            }));
        }
    }

    let request_id = format!("api-roles-{}", ulid::Ulid::new());
    let reply = db_round_trip(
        state,
        DbCommand::EntityAccess {
            target: target.clone(),
            action: EntityAccessAction::Load {
                peer_dids: Vec::new(),
            },
            request_id: request_id.clone(),
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    match reply {
        DbResult::EntityAccessFinished {
            result: Ok(snap), ..
        } => Ok(json!({
            "verse_id": verse_id,
            "scope": target.scope,
            "local_did": snap.local_did,
            "local_role": snap.local_role,
            "roles": snap.roles.iter().map(|(did, role)| json!({ "peer_did": did, "role": role })).collect::<Vec<_>>(),
            "grants": grants_to_json(&snap.grants),
        })),
        DbResult::EntityAccessFinished { result: Err(e), .. } => {
            Err(PeeringError::new(StatusCode::FORBIDDEN, e))
        }
        _ => Err(PeeringError::new(
            StatusCode::BAD_GATEWAY,
            "role load failed: unexpected db reply",
        )),
    }
}

pub(crate) async fn assign_role_core(
    state: &ApiState,
    claims: &ApiClaims,
    verse_id: &str,
    req: AssignRoleRequest,
) -> Result<Value, PeeringError> {
    let target = require_covered_verse_target(
        state,
        claims,
        verse_id,
        req.scope.as_deref(),
        "manager",
    )
    .await?;
    let _local_did = require_local_operator(state, claims)?;
    let peer_did = req.peer_did.trim().to_owned();
    if peer_did.is_empty() || peer_did.len() > 256 || peer_did.chars().any(char::is_control) {
        return Err(PeeringError::new(StatusCode::BAD_REQUEST, "invalid peer_did"));
    }
    let request_id = format!("api-assign-{}", ulid::Ulid::new());
    let reply = db_round_trip(
        state,
        DbCommand::EntityAccess {
            target: target.clone(),
            action: EntityAccessAction::Assign {
                peer_did: peer_did.clone(),
                role: req.role.clone(),
            },
            request_id,
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    match reply {
        DbResult::EntityAccessFinished {
            result: Ok(snap), ..
        } => {
            let effective = snap
                .roles
                .iter()
                .find(|(d, _)| d == &peer_did)
                .map(|(_, r)| r.as_str())
                .unwrap_or("none");
            Ok(json!({
                "verse_id": verse_id,
                "scope": target.scope,
                "peer_did": peer_did,
                "role": effective,
                "notice": snap.notice,
                "grants": grants_to_json(&snap.grants),
            }))
        }
        DbResult::EntityAccessFinished { result: Err(e), .. } => {
            Err(PeeringError::new(StatusCode::BAD_REQUEST, e))
        }
        _ => Err(PeeringError::new(
            StatusCode::BAD_GATEWAY,
            "role assignment failed: unexpected db reply",
        )),
    }
}

pub(crate) async fn revoke_role_core(
    state: &ApiState,
    claims: &ApiClaims,
    verse_id: &str,
    peer_did: &str,
    scope: Option<&str>,
) -> Result<Value, PeeringError> {
    let target = require_covered_verse_target(state, claims, verse_id, scope, "manager").await?;
    let _local_did = require_local_operator(state, claims)?;
    let peer_did = peer_did.trim().to_owned();
    if peer_did.is_empty() || peer_did.len() > 256 || peer_did.chars().any(char::is_control) {
        return Err(PeeringError::new(StatusCode::BAD_REQUEST, "invalid peer_did"));
    }
    let request_id = format!("api-revoke-{}", ulid::Ulid::new());
    let reply = db_round_trip(
        state,
        DbCommand::EntityAccess {
            target: target.clone(),
            action: EntityAccessAction::Revoke {
                peer_did: peer_did.clone(),
            },
            request_id,
        },
    )
    .await
    .map_err(|e| PeeringError::new(StatusCode::BAD_GATEWAY, e))?;

    match reply {
        DbResult::EntityAccessFinished {
            result: Ok(snap), ..
        } => {
            let effective = snap
                .roles
                .iter()
                .find(|(d, _)| d == &peer_did)
                .map(|(_, r)| r.as_str())
                .unwrap_or("none");
            Ok(json!({
                "verse_id": verse_id,
                "scope": target.scope,
                "peer_did": peer_did,
                "role_after": effective,
                "notice": snap.notice,
                "grants": grants_to_json(&snap.grants),
            }))
        }
        DbResult::EntityAccessFinished { result: Err(e), .. } => {
            Err(PeeringError::new(StatusCode::BAD_REQUEST, e))
        }
        _ => Err(PeeringError::new(
            StatusCode::BAD_GATEWAY,
            "role revocation failed: unexpected db reply",
        )),
    }
}

pub async fn list_roles(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> Response {
    match list_roles_core(&state, &claims, &verse_id, q.scope.as_deref()).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(err) => err.into_response(),
    }
}

pub async fn assign_role(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    Json(req): Json<AssignRoleRequest>,
) -> Response {
    match assign_role_core(&state, &claims, &verse_id, req).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(err) => err.into_response(),
    }
}

pub async fn revoke_role(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((verse_id, peer_did)): Path<(String, String)>,
    Query(q): Query<ScopeQuery>,
) -> Response {
    match revoke_role_core(&state, &claims, &verse_id, &peer_did, q.scope.as_deref()).await {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::peering::NodePeeringIdentity;
    use tokio::sync::broadcast;

    fn test_state(kp: &fe_identity::NodeKeypair) -> Arc<ApiState> {
        let (api_cmd_tx, _rx) = crossbeam::channel::bounded(8);
        let (tx_bcast, _) = broadcast::channel(8);
        let (ent_bcast, _) = broadcast::channel(8);
        Arc::new(ApiState {
            terrain_capture: Default::default(),
            api_cmd_tx,
            transform_broadcast_tx: tx_bcast,
            entity_change_tx: ent_bcast,
            verifying_key: kp.verifying_key(),
            revoked_jtis: Default::default(),
            blob_store: None,
            cors_origins: vec!["*".into()],
            db_reader: None,
            query_rate_limiter: tokio::sync::Mutex::new(Default::default()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
            elevated_query_allowed: false,
            upload_quota_bytes: 1024 * 1024,
            screenshot: crate::screenshot::ScreenshotSurface::disabled(),
            input: crate::input::InputSurface::disabled(),
        })
    }

    fn claims(sub: &str, scope: &str, max_role: &str) -> ApiClaims {
        ApiClaims {
            sub: sub.to_owned(),
            scope: scope.to_owned(),
            max_role: max_role.to_owned(),
            exp: u64::MAX / 2,
            iat: 0,
            jti: "jti-1".to_owned(),
            token_type: "api".to_owned(),
            client: None,
        }
    }

    #[tokio::test]
    async fn identity_strips_addresses_below_owner() {
        let kp = fe_identity::NodeKeypair::generate();
        let did = kp.to_did_key();
        let state = test_state(&kp);
        let (tx, rx) = crossbeam::channel::bounded::<PeeringCall>(4);
        std::thread::spawn(move || {
            while let Ok(call) = rx.recv() {
                if let PeeringCall::GetIdentity { reply, .. } = call {
                    let _ = reply.send(NodePeeringIdentity {
                        did: did.clone(),
                        endpoint_id: Some("ep1".into()),
                        direct_addrs: vec!["203.0.113.5:4000".into()],
                        home_relay: Some("https://relay.example".into()),
                        relay_mode: "default".into(),
                        relay_health: "healthy".into(),
                    });
                }
            }
        });

        let viewer = get_identity_core(&state, &claims(&kp.to_did_key(), "VERSE#v1", "viewer"), Some(&tx))
            .await
            .expect("viewer identity");
        assert!(viewer.direct_addrs.is_empty());
        assert!(viewer.home_relay.is_none());
        assert_eq!(viewer.endpoint_id.as_deref(), Some("ep1"));

        let owner = get_identity_core(&state, &claims(&kp.to_did_key(), "VERSE#v1", "owner"), Some(&tx))
            .await
            .expect("owner identity");
        assert_eq!(owner.direct_addrs, vec!["203.0.113.5:4000".to_string()]);
        assert_eq!(owner.home_relay.as_deref(), Some("https://relay.example"));
    }

    #[tokio::test]
    async fn join_and_invite_fail_closed_on_role_or_foreign_sub() {
        let kp = fe_identity::NodeKeypair::generate();
        let other = fe_identity::NodeKeypair::generate();
        let state = test_state(&kp);
        let (tx, _rx) = crossbeam::channel::bounded::<PeeringCall>(4);

        // Editor cannot join_verse (Owner required).
        let err = join_verse_core(
            &state,
            &claims(&kp.to_did_key(), "VERSE#01J00000000000000000000000", "editor"),
            Some(&tx),
            JoinVerseRequest {
                invite: "fe-join-v2:dummy".into(),
                wait: Some(false),
                timeout_secs: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);

        // Foreign subject even with Owner cannot join_verse as local node.
        let err = join_verse_core(
            &state,
            &claims(&other.to_did_key(), "VERSE#01J00000000000000000000000", "owner"),
            Some(&tx),
            JoinVerseRequest {
                invite: "fe-join-v2:dummy".into(),
                wait: Some(false),
                timeout_secs: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }
}
