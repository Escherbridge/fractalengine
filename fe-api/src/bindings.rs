//! Scoped extension bindings API (docs/extensions/bindings.md §9): list at an exact scope, the per-petal
//! effective view, put and clear, as REST routes and as core functions MCP calls (`list_bindings`,
//! `set_binding`, `clear_binding`). Rationale: `fe-api/AGENTS.md` §bindings (landed by I-API).
//!
//! fe-api never links the plugin host: the binary implements [`BindingHost`] (writes through
//! `DbCommand::PutBinding`, reads through `ListBindings` and its resolution cache). This module owns the
//! HTTP-facing rules: remote rate gate, token role ceiling and scope, uniform not-found, error mapping,
//! and "the local DID alone sees host state and offers".

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use fe_identity::api_token::ApiClaims;
use fe_runtime::extension_binding::{BindingMode, BindingRecord, BindingResolution, BINDING_SCHEMA_VERSION};
use fe_runtime::messages::{BindingRefusal, StoredBinding};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::extensions::{ExtError, REPLY_TIMEOUT};
use crate::rest::{authorized_petal_scope, covered_scope, resolve_fractal_scope, resolve_petal_scope, resolve_verse_scope};
use crate::server::ApiState;
use crate::types::ApiResponse;

/// Largest request body a binding write accepts (the record itself caps at 32 KiB in fe-runtime).
pub const MAX_BINDING_BODY_BYTES: usize = 64 * 1024;
/// Longest `runs_on` DID.
const MAX_DID_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Host seam
// ---------------------------------------------------------------------------

/// The eventual answer of a binding write: the stored row, or the DB plane's typed refusal.
pub type BindingWriteReply = oneshot::Receiver<Result<BindingRecord, BindingRefusal>>;
/// The eventual answer of a scope read: every judged version at the scope.
pub type BindingListReply = oneshot::Receiver<Result<Vec<StoredBinding>, BindingRefusal>>;

/// One hook point's winning binding at a petal, with this host's state for the local DID only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectivePoint {
    pub hook: String,
    pub key: String,
    /// The scope level's string the winning row sits at.
    pub scope: String,
    pub mode: BindingMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_req: Option<String>,
    /// This host's resolution (state and one offer). The host fills it; the API drops it for any caller that is not the local DID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<BindingResolution>,
}

/// What the binary provides: the binding plane behind the routes.
pub trait BindingHost: Send + Sync {
    /// The host's local DID (the identity binding rows are authored as).
    fn local_did(&self) -> String;
    /// A remote DID's request rate on binding routes (checked before any lookup).
    fn admit_remote(&self, _did: &str) -> bool {
        true
    }
    /// Send `DbCommand::PutBinding`. `caller_did` is the token's `sub`; the DB plane judges Manager+ for a
    /// remote DID and always for the local DID as author (bindings.md §4.2). `template.mode == Inherit` clears.
    fn put_binding(&self, caller_did: &str, template: BindingRecord) -> BindingWriteReply;
    /// Send `DbCommand::ListBindings` for exactly `scope`; `caller_did` is the token's `sub`.
    fn list_bindings(&self, caller_did: &str, scope: &str) -> BindingListReply;
    /// The effective view at a petal scope from the resolution cache. `None` = unknown petal, or a non-local
    /// `caller_did` below Viewer there (the uniform 404). Fill `resolution` for every point.
    fn effective(&self, petal_scope: &str, caller_did: &str) -> Option<Vec<EffectivePoint>>;
}

/// The host as an axum `Extension`.
#[derive(Clone)]
pub struct BindingHostHandle(pub Arc<dyn BindingHost>);

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// The scope a request addresses by ids (`verse_id`, then optional `fractal_id`, then optional `petal_id`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct BindingTarget {
    #[serde(default)]
    pub verse_id: String,
    #[serde(default)]
    pub fractal_id: Option<String>,
    #[serde(default)]
    pub petal_id: Option<String>,
}

/// `PUT /api/v1/bindings` body and MCP `set_binding` arguments.
#[derive(Debug, Clone, Deserialize)]
pub struct SetBindingRequest {
    #[serde(flatten)]
    pub target: BindingTarget,
    pub hook: String,
    #[serde(default)]
    pub key: String,
    pub mode: BindingMode,
    #[serde(default)]
    pub package_id: Option<String>,
    #[serde(default)]
    pub version_req: Option<String>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub runs_on: Option<String>,
}

/// `DELETE /api/v1/bindings` body and MCP `clear_binding` arguments.
#[derive(Debug, Clone, Deserialize)]
pub struct ClearBindingRequest {
    #[serde(flatten)]
    pub target: BindingTarget,
    pub hook: String,
    #[serde(default)]
    pub key: String,
}

// ---------------------------------------------------------------------------
// Errors and gates
// ---------------------------------------------------------------------------

fn error(status: u16, code: &'static str, message: impl Into<String>) -> ExtError {
    ExtError { status, code, message: message.into() }
}

/// The DB plane's stable refusal ids (bindings.md §4.2) as HTTP errors; unknown ids are a generic failure.
fn refusal_error(refusal: BindingRefusal) -> ExtError {
    match refusal.code.as_str() {
        "invalid-binding" => error(422, "invalid-binding", refusal.message),
        "unknown-hook" => error(422, "unknown-hook", refusal.message),
        "role-denied" => error(403, "role-denied", refusal.message),
        "policy-denied" => error(403, "policy-denied", refusal.message),
        // The uniform not-found never carries detail.
        "not-found" => ExtError::not_found(),
        _ => error(502, "failed", refusal.message),
    }
}

/// True for the host's own DID under an Owner token; every node-minted token carries the node DID as `sub`,
/// so the role ceiling is what separates the operator from a delegated token (fe-api/AGENTS.md §extensions).
fn is_local(host: &dyn BindingHost, claims: &ApiClaims) -> bool {
    claims.sub == host.local_did() && crate::auth::require_role_level(claims, fe_policy::RoleLevel::Owner).is_ok()
}

/// Every binding route runs this FIRST: a remote DID's request rate, before any lookup.
fn gate(host: &dyn BindingHost, claims: &ApiClaims) -> Result<(), ExtError> {
    if is_local(host, claims) || host.admit_remote(&claims.sub) {
        Ok(())
    } else {
        Err(ExtError::rate_limited("binding request rate limit"))
    }
}

/// The token's role ceiling must reach `floor`; a token that covers the scope but not the role is told so.
fn require_ceiling(claims: &ApiClaims, floor: fe_policy::RoleLevel, what: &str) -> Result<(), ExtError> {
    crate::auth::require_role_level(claims, floor)
        .map_err(|_| error(403, "role-denied", format!("{what} needs {}", floor_text(floor))))
}

fn floor_text(floor: fe_policy::RoleLevel) -> &'static str {
    match floor {
        fe_policy::RoleLevel::Manager => "Manager+",
        _ => "Viewer+",
    }
}

/// Wait for the DB plane under the reply deadline.
async fn await_reply<T>(reply: oneshot::Receiver<Result<T, BindingRefusal>>) -> Result<T, ExtError> {
    match tokio::time::timeout(REPLY_TIMEOUT, reply).await {
        Ok(Ok(result)) => result.map_err(refusal_error),
        Ok(Err(_)) => Err(error(503, "not-available", "the binding plane stopped before answering")),
        Err(_) => Err(error(504, "unknown-outcome", "the binding plane did not answer in time")),
    }
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

/// Resolve ids to the exact scope string the token covers, else `None` (the uniform 404). The DB-resolved scope
/// must agree with every claimed ancestor, so a mismatched id never reveals that another id exists.
pub async fn authorized_target_scope(state: &ApiState, claims: &ApiClaims, target: &BindingTarget) -> Option<String> {
    if target.verse_id.is_empty() {
        return None;
    }
    let resolved = if let Some(petal_id) = target.petal_id.as_deref() {
        resolve_petal_scope(state, petal_id).await
    } else if let Some(fractal_id) = target.fractal_id.as_deref() {
        resolve_fractal_scope(state, fractal_id).await
    } else {
        resolve_verse_scope(state, &target.verse_id).await
    };
    let scope = resolved.filter(|scope| scope_matches_target(scope, target))?;
    covered_scope(claims, Some(scope))
}

/// True when `scope` is exactly the level `target` names and every claimed id equals its chain.
fn scope_matches_target(scope: &str, target: &BindingTarget) -> bool {
    let Ok(parts) = fe_database::parse_scope(scope) else {
        return false;
    };
    parts.verse_id == target.verse_id
        && match (&target.fractal_id, &parts.fractal_id) {
            (Some(claimed), Some(actual)) => claimed == actual,
            (None, _) => true,
            (Some(_), None) => false,
        }
        && match (&target.petal_id, &parts.petal_id) {
            (Some(claimed), Some(actual)) => claimed == actual,
            (None, None) => true,
            _ => false,
        }
}

// ---------------------------------------------------------------------------
// Core (shared by REST and MCP)
// ---------------------------------------------------------------------------

/// `GET /api/v1/bindings` and MCP `list_bindings`: the judged versions at the exact `scope` (`None` = the uniform
/// 404: unknown, foreign or out of token scope). Viewer+; the DB plane re-checks a remote DID's role.
pub async fn list_bindings(host: &dyn BindingHost, claims: &ApiClaims, scope: Option<String>) -> Result<Value, ExtError> {
    gate(host, claims)?;
    list_after_gate(host, claims, scope).await
}

async fn list_after_gate(host: &dyn BindingHost, claims: &ApiClaims, scope: Option<String>) -> Result<Value, ExtError> {
    let scope = scope.ok_or_else(ExtError::not_found)?;
    require_ceiling(claims, fe_policy::RoleLevel::Viewer, "reading bindings")?;
    let mut versions = await_reply(host.list_bindings(&claims.sub, &scope)).await?;
    versions.sort_by(|a, b| {
        (&a.record.hook, &a.record.key, std::cmp::Reverse(a.record.written_at.parse::<u64>().unwrap_or(0)))
            .cmp(&(&b.record.hook, &b.record.key, std::cmp::Reverse(b.record.written_at.parse::<u64>().unwrap_or(0))))
    });
    let rows: Vec<Value> = versions.iter().map(stored_json).collect();
    Ok(json!({ "scope": scope, "bindings": rows }))
}

/// One version as the record's fields plus the judging flags.
fn stored_json(version: &StoredBinding) -> Value {
    let mut row = match serde_json::to_value(&version.record) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    row.insert("effective".into(), json!(version.effective));
    row.insert("admissible".into(), json!(version.admissible));
    row.insert("written_authorized".into(), json!(version.written_authorized));
    Value::Object(row)
}

/// `GET /api/v1/petals/{petal_id}/bindings/effective`: per point the winning scope, mode, package and version
/// requirement. Only the local DID also gets this host's state and offer (so a remote caller cannot probe
/// whether this host admits extensions). `petal_scope` is the token-authorized petal scope.
pub async fn effective_bindings(
    host: &dyn BindingHost,
    claims: &ApiClaims,
    petal_scope: Option<String>,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    effective_after_gate(host, claims, petal_scope)
}

fn effective_after_gate(host: &dyn BindingHost, claims: &ApiClaims, petal_scope: Option<String>) -> Result<Value, ExtError> {
    let scope = petal_scope.ok_or_else(ExtError::not_found)?;
    require_ceiling(claims, fe_policy::RoleLevel::Viewer, "reading bindings")?;
    let mut points = host.effective(&scope, &claims.sub).ok_or_else(ExtError::not_found)?;
    if !is_local(host, claims) {
        for point in &mut points {
            point.resolution = None;
        }
    }
    Ok(json!({ "petal_scope": scope, "points": points }))
}

/// `PUT /api/v1/bindings` and MCP `set_binding`: runs bindings.md §4.2 and returns the stored row. `scope` is
/// the token-authorized target (`None` = the uniform 404). Needs a Manager+ ceiling; the DB plane judges the DID.
pub async fn set_binding(
    host: &dyn BindingHost,
    claims: &ApiClaims,
    scope: Option<String>,
    request: SetBindingRequest,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    write_after_gate(host, claims, scope, build_set(&request)?).await
}

/// `DELETE /api/v1/bindings` and MCP `clear_binding`: writes `mode: inherit` (never a delete).
pub async fn clear_binding(
    host: &dyn BindingHost,
    claims: &ApiClaims,
    scope: Option<String>,
    request: ClearBindingRequest,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    write_after_gate(host, claims, scope, build_clear(&request)?).await
}

/// The record a `set` carries before the scope is attached; shape rules that need no state fail early (422).
fn build_set(request: &SetBindingRequest) -> Result<BindingRecord, ExtError> {
    if let Some(runs_on) = &request.runs_on {
        let valid = runs_on.starts_with("did:")
            && runs_on.len() <= MAX_DID_LEN
            && !runs_on.contains('/')
            && !runs_on.chars().any(char::is_whitespace);
        if !valid {
            return Err(error(422, "invalid-binding", "runs_on must be a DID"));
        }
    }
    Ok(BindingRecord {
        schema_version: BINDING_SCHEMA_VERSION,
        scope: String::new(),
        hook: request.hook.clone(),
        key: request.key.clone(),
        mode: request.mode,
        package_id: request.package_id.clone(),
        version_req: request.version_req.clone(),
        config: request.config.clone().unwrap_or_else(|| json!({})),
        runs_on: request.runs_on.clone(),
        author_did: String::new(),
        written_at: "0".into(),
    })
}

fn build_clear(request: &ClearBindingRequest) -> Result<BindingRecord, ExtError> {
    Ok(BindingRecord {
        schema_version: BINDING_SCHEMA_VERSION,
        scope: String::new(),
        hook: request.hook.clone(),
        key: request.key.clone(),
        mode: BindingMode::Inherit,
        package_id: None,
        version_req: None,
        config: json!({}),
        runs_on: None,
        author_did: String::new(),
        written_at: "0".into(),
    })
}

/// Scope, ceiling and shape, then the one DB command. `author_did` and `written_at` are host-stamped downstream.
async fn write_after_gate(
    host: &dyn BindingHost,
    claims: &ApiClaims,
    scope: Option<String>,
    mut template: BindingRecord,
) -> Result<Value, ExtError> {
    // Out of scope or unknown is the uniform 404 before any role detail is revealed.
    let scope = scope.ok_or_else(ExtError::not_found)?;
    require_ceiling(claims, fe_policy::RoleLevel::Manager, "writing a binding")?;
    template.scope = scope;
    template.author_did = claims.sub.clone();
    template
        .validate_shape()
        .map_err(|e| error(422, "invalid-binding", e.to_string()))?;
    let stored = await_reply(host.put_binding(&claims.sub, template)).await?;
    serde_json::to_value(&stored).map_err(|e| error(502, "failed", format!("stored binding is not JSON: {e}")))
}

// ---------------------------------------------------------------------------
// REST
// ---------------------------------------------------------------------------

/// The binding routes, to merge into the authenticated router when the binary provides a [`BindingHost`].
pub fn routes(host: Arc<dyn BindingHost>) -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/bindings", get(get_bindings).put(put_binding).delete(delete_binding))
        .route("/api/v1/petals/{petal_id}/bindings/effective", get(get_effective))
        .layer(Extension(BindingHostHandle(host)))
}

fn respond(result: Result<Value, ExtError>) -> Response {
    match result {
        Ok(data) => (StatusCode::OK, Json(ApiResponse::success(data))).into_response(),
        Err(e) => (e.status_code(), Json(ApiResponse::<Value>::error(e.message))).into_response(),
    }
}

/// Strict JSON object body within the size cap.
fn parse_body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ExtError> {
    if bytes.len() > MAX_BINDING_BODY_BYTES {
        return Err(error(413, "payload-too-large", "binding body exceeds 64 KiB"));
    }
    serde_json::from_slice(bytes).map_err(|e| ExtError::invalid(format!("invalid binding body: {e}")))
}

async fn get_bindings(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<BindingHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Query(target): Query<BindingTarget>,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return respond(Err(e));
    }
    let scope = authorized_target_scope(&state, &claims, &target).await;
    respond(list_after_gate(host.0.as_ref(), &claims, scope).await)
}

async fn get_effective(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<BindingHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return respond(Err(e));
    }
    let scope = authorized_petal_scope(&state, &claims, &petal_id).await;
    respond(effective_after_gate(host.0.as_ref(), &claims, scope))
}

async fn put_binding(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<BindingHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return respond(Err(e));
    }
    let request: SetBindingRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(e) => return respond(Err(e)),
    };
    let template = match build_set(&request) {
        Ok(template) => template,
        Err(e) => return respond(Err(e)),
    };
    let scope = authorized_target_scope(&state, &claims, &request.target).await;
    respond(write_after_gate(host.0.as_ref(), &claims, scope, template).await)
}

async fn delete_binding(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<BindingHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return respond(Err(e));
    }
    let request: ClearBindingRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(e) => return respond(Err(e)),
    };
    let template = match build_clear(&request) {
        Ok(template) => template,
        Err(e) => return respond(Err(e)),
    };
    let scope = authorized_target_scope(&state, &claims, &request.target).await;
    respond(write_after_gate(host.0.as_ref(), &claims, scope, template).await)
}
