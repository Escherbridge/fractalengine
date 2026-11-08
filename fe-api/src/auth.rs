use std::{collections::HashSet, sync::Arc};

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

    let started = std::time::Instant::now();
    let claims = authenticate(&state, token).await?;
    // QA-0161: the durable credential read is a per-request DB read; debug timing splits it out.
    tracing::debug!(authenticate_ms = started.elapsed().as_secs_f64() * 1000.0, path = %req.uri().path(), "API credential check timing");

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

const FEED_UNAVAILABLE: &str = "\0credential-feed-unavailable";

/// Verify a credential against the signing key and current durable validity.
pub async fn authenticate(
    state: &crate::server::ApiState,
    token: &str,
) -> Result<ApiClaims, StatusCode> {
    let claims =
        verify_api_token(token, &state.verifying_key).map_err(|_| StatusCode::UNAUTHORIZED)?;
    require_current_credential(state, &claims).await?;
    Ok(claims)
}

async fn durable_snapshot(
    state: &crate::server::ApiState,
    jti: &str,
) -> Result<Option<Vec<serde_json::Value>>, StatusCode> {
    let Some(db) = &state.db_reader else {
        return Ok(None);
    };
    let query = async {
        let mut response = db
            .query(fe_database::api_auth::CREDENTIAL_SQL)
            .bind(("credential_jti", jti.to_owned()))
            .await?
            .check()?;
        let rows: Vec<serde_json::Value> = response.take(1)?;
        fe_database::api_auth::generation_from_rows(&rows)?;
        let tokens = rows[0]["tokens"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid credential snapshot"))?;
        anyhow::ensure!(tokens.len() <= 1, "Ambiguous credential snapshot");
        for token in tokens {
            anyhow::ensure!(
                token["sub"].is_string() && token["revoked"].is_boolean(),
                "Invalid credential metadata"
            );
        }
        Ok::<_, anyhow::Error>(rows)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), query).await {
        Ok(Ok(rows)) => Ok(Some(rows)),
        _ => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn require_unexpired(claims: &ApiClaims) -> Result<(), StatusCode> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .as_secs();
    if claims.exp <= now {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn require_revocation_hints(
    revoked: &HashSet<String>,
    jti: &str,
    has_durable_reader: bool,
) -> Result<(), StatusCode> {
    if revoked.contains(jti) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !has_durable_reader && revoked.contains(FEED_UNAVAILABLE) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(())
}

/// Revalidate a verified credential before a protected action or async response.
pub async fn require_current_credential(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
) -> Result<(), StatusCode> {
    require_unexpired(claims)?;
    if claims.jti.is_empty() || claims.jti.len() > 128 || claims.jti.chars().any(char::is_control) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    {
        let revoked = state.revoked_jtis.read().await;
        require_revocation_hints(&revoked, &claims.jti, state.db_reader.is_some())?;
    }
    if let Some(rows) = durable_snapshot(state, &claims.jti).await? {
        let generation = fe_database::api_auth::generation_from_rows(&rows)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !fe_database::api_auth::accepts_jti(generation, &claims.jti) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        if let Some(token) = rows[0]["tokens"]
            .as_array()
            .and_then(|tokens| tokens.first())
        {
            if token["sub"].as_str() != Some(claims.sub.as_str())
                || token["revoked"].as_bool() != Some(false)
            {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }
    {
        let revoked = state.revoked_jtis.read().await;
        require_revocation_hints(&revoked, &claims.jti, state.db_reader.is_some())?;
    }
    require_unexpired(claims)
}

/// Complete a terrain capture after its separate durable direct/worker read.
pub(crate) async fn require_terrain_credential_hints(
    state: &crate::server::ApiState, claims: &ApiClaims,
) -> Result<(), StatusCode> {
    let revoked = state.revoked_jtis.read().await;
    require_revocation_hints(&revoked, &claims.jti, true)?;
    require_unexpired(claims)
}

pub(crate) async fn validate_auth_startup(
    state: &crate::server::ApiState,
) -> Result<(), StatusCode> {
    durable_snapshot(state, "").await.map(|_| ())
}

/// Consume revocation hints, recovering dropped notifications from durable authority.
pub async fn listen_for_revocations(
    state: Arc<crate::server::ApiState>,
    mut receiver: tokio::sync::broadcast::Receiver<String>,
) {
    loop {
        match receiver.recv().await {
            Ok(jti) => {
                state.revoked_jtis.write().await.insert(jti);
            }
            Err(error) => {
                state
                    .revoked_jtis
                    .write()
                    .await
                    .insert(FEED_UNAVAILABLE.into());
                tracing::warn!(
                    "Credential notification stream interrupted; durable revalidation required"
                );
                if state.db_reader.is_none() {
                    return;
                }
                while validate_auth_startup(&state).await.is_err() {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                state.revoked_jtis.write().await.remove(FEED_UNAVAILABLE);
                if matches!(error, tokio::sync::broadcast::error::RecvError::Closed) {
                    return;
                }
            }
        }
    }
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
            client: None,
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
                "hyphenated foreign verse denied",
                require_scope(&claims("VERSE#v1", "owner"), "VERSE#v1-other-FRACTAL#f1"),
                forbidden,
            ),
            (
                "hyphenated foreign fractal denied",
                require_scope(
                    &claims("VERSE#v1-FRACTAL#f1", "owner"),
                    "VERSE#v1-FRACTAL#f1-other-PETAL#p1",
                ),
                forbidden,
            ),
            (
                "hyphenated foreign petal denied",
                require_scope(
                    &claims(V1_PETAL, "owner"),
                    "VERSE#v1-FRACTAL#f1-PETAL#p1-other",
                ),
                forbidden,
            ),
            (
                "hyphenated ancestor covers its descendant",
                require_scope(
                    &claims("VERSE#verse-abc-FRACTAL#fractal-xyz", "viewer"),
                    "VERSE#verse-abc-FRACTAL#fractal-xyz-PETAL#petal-123",
                ),
                Ok(()),
            ),
            (
                "malformed descendant denied",
                require_scope(&claims("VERSE#v1", "owner"), "VERSE#v1-FRACTAL#f1-PETAL#"),
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
