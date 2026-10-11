use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use fe_database::RoleLevel;
use fe_identity::api_token::{verify_api_token, ApiClaims};
use fe_policy::{
    Action, AllOf, AuthContext, Decision, Policy, PolicyEngine, RoleFloorPolicy, Scope,
    TokenScopePolicy,
};

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

    let claims =
        verify_api_token(token, &state.verifying_key).map_err(|_| StatusCode::UNAUTHORIZED)?;

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
// Scope enforcement helpers — every check evaluates through fe-policy
// (see fe-api/AGENTS.md §auth-policy).
// ---------------------------------------------------------------------------

/// The single `AuthContext` builder for an API token (role = its `max_role` ceiling).
pub(crate) fn api_token_context(claims: &ApiClaims) -> AuthContext {
    AuthContext::ApiToken {
        subject: claims.sub.clone(),
        scopes: vec![claims.scope.clone()],
        role: RoleLevel::from(claims.max_role.as_str()),
    }
}

/// Evaluate `policy` for the token; any denial maps to `FORBIDDEN`.
fn enforce(policy: &dyn Policy, claims: &ApiClaims, resource: &Scope) -> Result<(), StatusCode> {
    match policy.evaluate(&api_token_context(claims), &Action::Read, resource) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(StatusCode::FORBIDDEN),
    }
}

/// Check that the token's `max_role` meets or exceeds the required role level.
pub fn require_role(claims: &ApiClaims, min_role: &str) -> Result<(), StatusCode> {
    require_role_level(claims, RoleLevel::from(min_role))
}

/// Typed form of [`require_role`] — a typo'd role string parses to
/// `RoleLevel::None` (admit-all), so tables use this instead.
pub fn require_role_level(claims: &ApiClaims, required: RoleLevel) -> Result<(), StatusCode> {
    enforce(&RoleFloorPolicy(required), claims, &Scope::global())
}

/// Check that the token's scope covers the given resource scope.
///
/// A verse-level token (`VERSE#v1`) covers all fractals and petals within
/// that verse. Returns `FORBIDDEN` if the token scope doesn't contain the
/// resource scope.
pub fn require_scope(claims: &ApiClaims, resource_scope: &str) -> Result<(), StatusCode> {
    enforce(&TokenScopePolicy, claims, &Scope::new(resource_scope))
}

/// Combined role + scope check: one `AllOf`, evaluated through a
/// `PolicyEngine` so a denial is warn-logged once.
pub fn require_role_and_scope(
    claims: &ApiClaims,
    min_role: &str,
    resource_scope: &str,
) -> Result<(), StatusCode> {
    let engine = PolicyEngine::new().with_policy(Arc::new(AllOf::new(vec![
        Arc::new(RoleFloorPolicy(RoleLevel::from(min_role))),
        Arc::new(TokenScopePolicy),
    ])));
    match engine.evaluate(
        &api_token_context(claims),
        &Action::Read,
        &Scope::new(resource_scope),
    ) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(StatusCode::FORBIDDEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1_PETAL: &str = "VERSE#v1-FRACTAL#f1-PETAL#p1";

    fn claims(scope: &str, role: &str) -> ApiClaims {
        ApiClaims {
            sub: "did:key:z6MkAuthTest".to_string(),
            scope: scope.to_string(),
            max_role: role.to_string(),
            token_type: "api".to_string(),
            iat: 0,
            exp: u64::MAX,
            jti: "jti-auth-test".to_string(),
        }
    }

    #[test]
    fn helper_table_evaluates_through_policies() {
        let forbidden = Err(StatusCode::FORBIDDEN);
        // (case, result, expected)
        let table = [
            (
                "viewer denied editor floor",
                require_role_level(&claims("VERSE#v1", "viewer"), RoleLevel::Editor),
                forbidden,
            ),
            (
                "owner floor denies manager",
                require_role_level(&claims("VERSE#v1", "manager"), RoleLevel::Owner),
                forbidden,
            ),
            (
                "editor meets editor floor",
                require_role_level(&claims("VERSE#v1", "editor"), RoleLevel::Editor),
                Ok(()),
            ),
            (
                "foreign verse scope denied",
                require_scope(&claims("VERSE#v2", "owner"), V1_PETAL),
                forbidden,
            ),
            (
                "prefix-trick verse denied",
                require_scope(&claims("VERSE#v", "owner"), V1_PETAL),
                forbidden,
            ),
            (
                "in-scope token covers its petal",
                require_scope(&claims("VERSE#v1", "viewer"), V1_PETAL),
                Ok(()),
            ),
            (
                "in-scope editor allowed",
                require_role_and_scope(&claims("VERSE#v1", "editor"), "editor", V1_PETAL),
                Ok(()),
            ),
            (
                "in-scope viewer denied write role",
                require_role_and_scope(&claims("VERSE#v1", "viewer"), "editor", V1_PETAL),
                forbidden,
            ),
            (
                "foreign editor denied",
                require_role_and_scope(&claims("VERSE#v2", "editor"), "editor", V1_PETAL),
                forbidden,
            ),
        ];
        for (case, got, want) in table {
            assert_eq!(got, want, "{case}");
        }
    }

    #[test]
    fn api_token_context_carries_the_token_identity() {
        let ctx = api_token_context(&claims("VERSE#v1", "manager"));
        assert_eq!(
            ctx,
            AuthContext::ApiToken {
                subject: "did:key:z6MkAuthTest".to_string(),
                scopes: vec!["VERSE#v1".to_string()],
                role: RoleLevel::Manager,
            }
        );
    }
}
