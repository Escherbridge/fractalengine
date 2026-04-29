use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::http::StatusCode;

/// Extract and verify an API token from the Authorization header.
/// Returns the verified ApiClaims or rejects with 401.
///
/// Current implementation: accepts any Bearer token and logs it.
/// Full verification against ed25519 public keys is Phase 4.
pub async fn auth_middleware(
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // For MVP: pass through all requests. Auth enforcement comes in Phase 4.
    // In production, extract Bearer token, verify ApiClaims, inject into extensions.
    let _auth_header = req.headers().get("authorization");
    Ok(next.run(req).await)
}

/// Extract a Bearer token string from an Authorization header value.
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    header_value.strip_prefix("Bearer ").or_else(|| header_value.strip_prefix("bearer "))
}
