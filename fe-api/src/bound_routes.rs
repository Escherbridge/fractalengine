//! Bound API routes (docs/extensions/bindings.md §6 `api.routes`, §8.2): the stable alias
//! `/api/v1/petals/{petal_id}/apis/{key}/{endpoint}[/{sub_path}]` that reaches whichever package the petal's
//! `api.routes` binding for `key` resolves to. Rationale: `fe-api/AGENTS.md` §bindings (landed by I-API).
//!
//! Order per request: bearer auth (outer layer), remote rate gate, `authorized_petal_scope` (uniform 404),
//! resolution through [`BindingHost::effective`], then the package ingress core
//! ([`crate::extensions::ingress`]) which applies the endpoint's role floor, rate, size and content-type
//! checks and strips credential headers. An unresolved binding answers 503 to Viewer+ and 404 otherwise.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Json, Router};
use fe_identity::api_token::ApiClaims;
use fe_runtime::extension_binding::BindingResolution;
use serde_json::{json, Value};

use crate::bindings::{BindingHost, BindingHostHandle};
use crate::extensions::{
    gate, ExtError, ExtensionHost, ExtensionHostHandle, IngressAnswer, IngressRefusal, IngressRequestParts,
    MAX_INGRESS_BODY_BYTES,
};
use crate::rest::authorized_petal_scope;
use crate::server::ApiState;
use crate::types::ApiResponse;

/// The hook id the alias serves (bindings.md §6).
const API_ROUTES_HOOK: &str = "api.routes";

/// What the alias does for one request once the petal is authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// An instance of this package serves the key on this peer.
    Package(String),
    /// A binding exists but cannot run here (the 503 body, no offer).
    Unresolved { package_id: String, reason: String },
    /// No extension serves the key here: unknown key, built-in, or placed on another peer.
    NoRoute,
}

/// The alias router; `I-API` merges it into the authenticated router beside `extensions::routes`.
pub fn router(binding_host: Arc<dyn BindingHost>, extension_host: Arc<dyn ExtensionHost>) -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/petals/{petal_id}/apis/{key}/{endpoint}", any(any_bound_root))
        .route("/api/v1/petals/{petal_id}/apis/{key}/{endpoint}/{*sub_path}", any(any_bound))
        .layer(Extension(BindingHostHandle(binding_host)))
        .layer(Extension(ExtensionHostHandle(extension_host)))
}

/// Pick the target for `key` from the petal's effective points.
pub fn route_target(points: &[crate::bindings::EffectivePoint], key: &str) -> RouteTarget {
    let Some(point) = points.iter().find(|p| p.hook == API_ROUTES_HOOK && p.key == key) else {
        return RouteTarget::NoRoute;
    };
    match point.resolution.as_ref() {
        Some(BindingResolution::Resolved { package_id, .. }) => RouteTarget::Package(package_id.clone()),
        Some(BindingResolution::Unresolved { package_id, reason, .. }) => RouteTarget::Unresolved {
            package_id: package_id.clone(),
            reason: serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "unresolved".into()),
        },
        _ => RouteTarget::NoRoute,
    }
}

/// Outcome of the alias core: the guest's answer, or the 503 body.
#[derive(Debug)]
pub enum BoundOutcome {
    Answer(IngressAnswer),
    Unresolved { package_id: String, reason: String },
}

/// Core of the alias, shared by tests and the handler. `petal_scope` is the token-authorized petal scope
/// (`None` = the uniform 404); `parts.package_id` is filled here from the resolution.
pub async fn bound_ingress(
    binding_host: &dyn BindingHost,
    extension_host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_scope: Option<String>,
    key: &str,
    mut parts: IngressRequestParts,
) -> Result<BoundOutcome, ExtError> {
    gate(extension_host, claims)?;
    let scope = petal_scope.ok_or_else(ExtError::not_found)?;
    // A token below Viewer reads as unknown, never as forbidden.
    crate::auth::require_role_level(claims, fe_policy::RoleLevel::Viewer).map_err(|_| ExtError::not_found())?;
    // `None` = unknown petal or a non-local caller below Viewer there.
    let points = binding_host.effective(&scope, &claims.sub).ok_or_else(ExtError::not_found)?;
    match route_target(&points, key) {
        RouteTarget::NoRoute => Err(ExtError::not_found()),
        RouteTarget::Unresolved { package_id, reason } => Ok(BoundOutcome::Unresolved { package_id, reason }),
        RouteTarget::Package(package_id) => {
            parts.package_id = package_id;
            // Role floor, rate, size, content type and the credential strip live in the ingress core.
            crate::extensions::ingress(extension_host, claims, true, parts).await.map(BoundOutcome::Answer)
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn any_bound_root(
    state: State<Arc<ApiState>>,
    bindings: Extension<BindingHostHandle>,
    extensions: Extension<ExtensionHostHandle>,
    claims: Extension<ApiClaims>,
    Path((petal_id, key, endpoint)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    serve(state, bindings, extensions, claims, petal_id, key, endpoint, String::new(), request).await
}

async fn any_bound(
    state: State<Arc<ApiState>>,
    bindings: Extension<BindingHostHandle>,
    extensions: Extension<ExtensionHostHandle>,
    claims: Extension<ApiClaims>,
    Path((petal_id, key, endpoint, sub_path)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    serve(state, bindings, extensions, claims, petal_id, key, endpoint, sub_path, request).await
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    State(state): State<Arc<ApiState>>,
    Extension(bindings): Extension<BindingHostHandle>,
    Extension(extensions): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    petal_id: String,
    key: String,
    endpoint: String,
    sub_path: String,
    request: Request,
) -> Response {
    if let Err(e) = gate(extensions.0.as_ref(), &claims) {
        return error_response(e);
    }
    // Websocket aliases are not part of v1; the package mount serves accept-mode sockets.
    let wants_upgrade = request
        .headers()
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if wants_upgrade {
        return error_response(ExtError {
            status: 501,
            code: "not-implemented",
            message: "websocket upgrade is not available on bound routes".into(),
        });
    }
    let (head, body) = request.into_parts();
    let content_type = head
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let headers: Vec<(String, String)> = head
        .headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
        .collect();
    let query = head.uri.query().map(url_pairs).unwrap_or_default();
    let body = match read_limited(body).await {
        Ok(bytes) => bytes,
        Err(e) => return error_response(e),
    };
    let petal_scope = authorized_petal_scope(&state, &claims, &petal_id).await;
    let parts = IngressRequestParts {
        petal_id,
        package_id: String::new(),
        endpoint,
        method: head.method.as_str().to_string(),
        sub_path,
        query,
        headers,
        content_type,
        body,
    };
    match bound_ingress(bindings.0.as_ref(), extensions.0.as_ref(), &claims, petal_scope, &key, parts).await {
        Ok(BoundOutcome::Answer(answer)) => answer_response(answer),
        Ok(BoundOutcome::Unresolved { package_id, reason }) => unresolved_response(&package_id, &reason),
        Err(e) if e.code == "no-reply" => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error_response(e),
    }
}

fn error_response(e: ExtError) -> Response {
    (e.status_code(), Json(ApiResponse::<Value>::error(e.message))).into_response()
}

/// 503 with the bindings.md §8.2 body; never carries the local-only offer.
fn unresolved_response(package_id: &str, reason: &str) -> Response {
    let body = json!({ "error": "binding-unresolved", "package_id": package_id, "reason": reason });
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

/// Read the body, refusing anything over the ingress ceiling (413).
async fn read_limited(body: Body) -> Result<Vec<u8>, ExtError> {
    let too_large = || ExtError::from(IngressRefusal::PayloadTooLarge);
    match axum::body::to_bytes(body, MAX_INGRESS_BODY_BYTES + 1).await {
        Ok(bytes) if bytes.len() <= MAX_INGRESS_BODY_BYTES => Ok(bytes.to_vec()),
        _ => Err(too_large()),
    }
}

/// Split `a=b&c=d` with `+` and `%XX` decoding (mirrors the package mount).
fn url_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A guest's answer as an HTTP response (status outside 200-599 is a 502; never sniffed or run as a document).
fn answer_response(answer: IngressAnswer) -> Response {
    let status = StatusCode::from_u16(answer.status)
        .ok()
        .filter(|status| (200..=599).contains(&status.as_u16()))
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, answer.body).into_response();
    let headers = response.headers_mut();
    if let Some(content_type) = answer.content_type.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
        headers.insert(HeaderName::from_static("content-type"), content_type);
    }
    headers.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    headers.insert(HeaderName::from_static("content-security-policy"), HeaderValue::from_static("sandbox"));
    response
}
