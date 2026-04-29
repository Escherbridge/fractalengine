//! API token minting and verification for the Realtime API Gateway.
//!
//! `ApiClaims` is a separate JWT type from `FractalClaims` — it supports
//! longer TTLs (up to 30 days), scope-based authorization at any hierarchy
//! level, and includes a `jti` for revocation tracking.

use crate::keypair::NodeKeypair;
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

fn ensure_crypto_provider() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

/// Maximum API token TTL: 30 days.
pub const MAX_API_TOKEN_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// JWT claims for API access tokens.
///
/// Distinct from `FractalClaims`:
/// - Longer TTL (up to 30 days vs 300 seconds)
/// - Scope-based access at any hierarchy level (not just petal)
/// - Token type discriminator for middleware routing
/// - `jti` for server-side revocation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiClaims {
    /// `did:key:z6Mk...` of the issuing node.
    pub sub: String,
    /// Always `"api"` — distinguishes from session tokens.
    pub token_type: String,
    /// Hierarchical scope string: `"VERSE#v1"`, `"VERSE#v1-FRACTAL#f1"`, etc.
    pub scope: String,
    /// Maximum role this token grants (e.g., `"editor"`, `"viewer"`).
    pub max_role: String,
    /// Issued-at (UNIX seconds).
    pub iat: u64,
    /// Expiration (UNIX seconds).
    pub exp: u64,
    /// Unique token ID for revocation tracking.
    pub jti: String,
}

/// Mint a new API token signed with the node's ed25519 keypair.
///
/// # Errors
/// Returns an error if `ttl_secs` exceeds `MAX_API_TOKEN_TTL_SECS` or if
/// signing fails.
pub fn mint_api_token(
    keypair: &NodeKeypair,
    scope: &str,
    max_role: &str,
    ttl_secs: u64,
    jti: &str,
) -> anyhow::Result<String> {
    ensure_crypto_provider();
    if ttl_secs > MAX_API_TOKEN_TTL_SECS {
        anyhow::bail!("API token TTL exceeds maximum of 30 days");
    }
    if scope.is_empty() {
        anyhow::bail!("API token scope must not be empty");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = ApiClaims {
        sub: keypair.to_did_key(),
        token_type: "api".to_string(),
        scope: scope.to_string(),
        max_role: max_role.to_string(),
        iat: now,
        exp: now + ttl_secs,
        jti: jti.to_string(),
    };
    let header = Header::new(Algorithm::EdDSA);
    let encoding_key = EncodingKey::from_ed_der(&keypair.pkcs8_der());
    encode(&header, &claims, &encoding_key).map_err(Into::into)
}

/// Verify an API token and extract its claims.
///
/// Checks signature, expiration, and that `token_type == "api"`.
pub fn verify_api_token(
    token: &str,
    issuer_pub_key: &VerifyingKey,
) -> anyhow::Result<ApiClaims> {
    ensure_crypto_provider();
    let raw = issuer_pub_key.to_bytes().to_vec();
    let decoding_key = DecodingKey::from_ed_der(&raw);
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.validate_exp = true;
    validation.required_spec_claims = std::collections::HashSet::new();
    let data = decode::<ApiClaims>(token, &decoding_key, &validation)?;
    if data.claims.token_type != "api" {
        anyhow::bail!("token_type is not 'api'");
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::NodeKeypair;

    #[test]
    fn mint_and_verify_api_token() {
        let kp = NodeKeypair::generate();
        let token = mint_api_token(&kp, "VERSE#v1", "editor", 3600, "jti-001").unwrap();
        let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
        assert_eq!(claims.scope, "VERSE#v1");
        assert_eq!(claims.max_role, "editor");
        assert_eq!(claims.token_type, "api");
        assert_eq!(claims.jti, "jti-001");
    }

    #[test]
    fn ttl_limit_enforced() {
        let kp = NodeKeypair::generate();
        let result = mint_api_token(&kp, "VERSE#v1", "viewer", MAX_API_TOKEN_TTL_SECS + 1, "j1");
        assert!(result.is_err());
    }

    #[test]
    fn empty_scope_rejected() {
        let kp = NodeKeypair::generate();
        let result = mint_api_token(&kp, "", "viewer", 3600, "j1");
        assert!(result.is_err());
    }

    #[test]
    fn session_token_rejected_as_api() {
        let kp = NodeKeypair::generate();
        // Mint a session token (FractalClaims) and try to verify as API
        let session = crate::jwt::mint_session_token(&kp, "petal-1", "admin", 300).unwrap();
        let result = verify_api_token(&session, &kp.verifying_key());
        assert!(result.is_err());
    }

    #[test]
    fn tampered_token_rejected() {
        let kp = NodeKeypair::generate();
        let mut token = mint_api_token(&kp, "VERSE#v1", "editor", 3600, "j1").unwrap();
        let last = token.pop().unwrap();
        token.push(if last == 'a' { 'b' } else { 'a' });
        assert!(verify_api_token(&token, &kp.verifying_key()).is_err());
    }
}
