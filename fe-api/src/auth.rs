use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use fe_database::RoleLevel;
use fe_identity::api_token::{verify_api_token, ApiClaims};

/// Extract Bearer token, verify JWT, inject ApiClaims into request extensions.
pub async fn auth_middleware(
    State(state): State<Arc<crate::server::ApiState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer_token);

    let Some(token) = header else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let claims = verify_api_token(token, &state.verifying_key)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Check revocation cache
    {
        let revoked = state.revoked_jtis.read().await;
        if revoked.contains(&claims.jti) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

/// Extract a Bearer token string from an Authorization header value.
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    header_value
        .strip_prefix("Bearer ")
        .or_else(|| header_value.strip_prefix("bearer "))
}

// ---------------------------------------------------------------------------
// Scope enforcement helpers
// ---------------------------------------------------------------------------

/// Check that the token's `max_role` meets or exceeds the required role level.
///
/// Uses `fe_database::RoleLevel` as the single source of truth for role ranking.
pub fn require_role(claims: &ApiClaims, min_role: &str) -> Result<(), StatusCode> {
    let actual = RoleLevel::from(claims.max_role.as_str());
    let required = RoleLevel::from(min_role);
    if !actual.is_at_least(required) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Check that the token's scope covers the given resource scope.
///
/// A verse-level token (`VERSE#v1`) covers all fractals and petals within
/// that verse. Returns `FORBIDDEN` if the token scope doesn't contain the
/// resource scope.
pub fn require_scope(claims: &ApiClaims, resource_scope: &str) -> Result<(), StatusCode> {
    if !fe_database::scope_contains(&claims.scope, resource_scope) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Combined role + scope check.
pub fn require_role_and_scope(
    claims: &ApiClaims,
    min_role: &str,
    resource_scope: &str,
) -> Result<(), StatusCode> {
    require_role(claims, min_role)?;
    require_scope(claims, resource_scope)?;
    Ok(())
}
