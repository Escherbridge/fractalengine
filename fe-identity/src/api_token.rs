//! API token minting and verification for the Realtime API Gateway
//! (design: fe-identity/src/AGENTS.md §api-token).

use crate::keypair::NodeKeypair;
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

fn ensure_crypto_provider() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

/// Maximum API token TTL: 30 days.
pub const MAX_API_TOKEN_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// JWT claims for API access tokens — deliberately distinct from
/// `FractalClaims` (rationale: fe-identity/src/AGENTS.md §api-token).
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
    /// Ed25519 `did:key` naming the API client this token was minted for.
    /// Selects the upload owner key only — never an authorization subject
    /// (fe-identity/src/AGENTS.md §api-token "client claim"). `None` = a
    /// legacy, node-level token; absent from the encoded JWT in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

/// Longest accepted `client` claim (an ed25519 `did:key` is 56 bytes).
pub const MAX_CLIENT_DID_LEN: usize = 128;

/// Check that `did` is a canonical ed25519 `did:key` no longer than
/// [`MAX_CLIENT_DID_LEN`] — the `client` claim grammar, enforced at mint and
/// again at verify.
pub fn validate_client_did(did: &str) -> anyhow::Result<()> {
    if did.is_empty() {
        anyhow::bail!("client DID must not be empty");
    }
    if did.len() > MAX_CLIENT_DID_LEN {
        anyhow::bail!("client DID exceeds {MAX_CLIENT_DID_LEN} bytes");
    }
    let key = crate::did_key::public_key_from_did_key(did)
        .map_err(|e| anyhow::anyhow!("client DID is not an ed25519 did:key: {e}"))?;
    // Re-encode: rejects any non-canonical spelling of the same key, so one
    // client can never form two owner buckets.
    if crate::did_key::did_key_from_public_key(&key) != did {
        anyhow::bail!("client DID is not in canonical did:key form");
    }
    Ok(())
}

/// A fresh client identity: the `did:key` of a newly generated ed25519 key
/// whose secret half is dropped immediately (only the public DID is kept).
pub fn new_client_did() -> String {
    NodeKeypair::generate().to_did_key()
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
    if ttl_secs > MAX_API_TOKEN_TTL_SECS {
        anyhow::bail!("API token TTL exceeds maximum of 30 days");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    mint_api_token_at(keypair, scope, max_role, now, now + ttl_secs, jti)
}

/// The production mint doors' one client rule: a requested DID must pass
/// [`validate_client_did`]; no request (`None`) yields [`new_client_did`].
pub fn resolve_client_did(requested: Option<&str>) -> anyhow::Result<String> {
    match requested {
        Some(did) => {
            validate_client_did(did)?;
            Ok(did.to_string())
        }
        None => Ok(new_client_did()),
    }
}

/// Mint an API token carrying a `client` claim. `client_did` is validated
/// with [`validate_client_did`] before anything is signed. The `sub` stays the
/// node DID — the client never changes what the token may do.
///
/// # Errors
/// As [`mint_api_token`], plus a malformed `client_did`.
pub fn mint_client_api_token(
    keypair: &NodeKeypair,
    scope: &str,
    max_role: &str,
    ttl_secs: u64,
    jti: &str,
    client_did: &str,
) -> anyhow::Result<String> {
    validate_client_did(client_did)?;
    if ttl_secs > MAX_API_TOKEN_TTL_SECS {
        anyhow::bail!("API token TTL exceeds maximum of 30 days");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    mint_with_client(
        keypair,
        scope,
        max_role,
        now,
        now + ttl_secs,
        jti,
        Some(client_did.to_string()),
    )
}

/// Like [`mint_api_token`], with explicit `iat`/`exp` (UNIX seconds) instead
/// of a TTL relative to "now". Lets callers mint a token that is already
/// expired, or expires in under a second, without waiting on the wall clock
/// — the harness/test lane for token-lifecycle coverage (`verify_api_token`'s
/// `jsonwebtoken` validator applies its own small leeway around `exp`, so a
/// caller asserting expiry should pick an `exp` comfortably in the past
/// rather than relying on TTL + sleep). The same TTL cap as [`mint_api_token`]
/// applies to the `exp - iat` span, and `exp` must not precede `iat`; a span
/// entirely in the past is fine (expired-token tests).
pub fn mint_api_token_at(
    keypair: &NodeKeypair,
    scope: &str,
    max_role: &str,
    iat: u64,
    exp: u64,
    jti: &str,
) -> anyhow::Result<String> {
    mint_with_client(keypair, scope, max_role, iat, exp, jti, None)
}

/// Shared signer for the legacy and client mints.
fn mint_with_client(
    keypair: &NodeKeypair,
    scope: &str,
    max_role: &str,
    iat: u64,
    exp: u64,
    jti: &str,
    client: Option<String>,
) -> anyhow::Result<String> {
    ensure_crypto_provider();
    if scope.is_empty() {
        anyhow::bail!("API token scope must not be empty");
    }
    let Some(span) = exp.checked_sub(iat) else {
        anyhow::bail!("API token exp must not precede iat");
    };
    if span > MAX_API_TOKEN_TTL_SECS {
        anyhow::bail!("API token TTL exceeds maximum of 30 days");
    }
    let claims = ApiClaims {
        sub: keypair.to_did_key(),
        token_type: "api".to_string(),
        scope: scope.to_string(),
        max_role: max_role.to_string(),
        iat,
        exp,
        jti: jti.to_string(),
        client,
    };
    let header = Header::new(Algorithm::EdDSA);
    let encoding_key = EncodingKey::from_ed_der(&keypair.pkcs8_der());
    encode(&header, &claims, &encoding_key).map_err(Into::into)
}

/// Verify an API token and extract its claims.
///
/// Checks signature, expiration, that `token_type == "api"`, and that a
/// present `client` claim passes [`validate_client_did`].
pub fn verify_api_token(token: &str, issuer_pub_key: &VerifyingKey) -> anyhow::Result<ApiClaims> {
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
    if let Some(client) = data.claims.client.as_deref() {
        validate_client_did(client)?;
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

    /// DEC-C21 M9: the explicit-timestamp mint enforces the same TTL cap on
    /// `exp - iat` and rejects `exp < iat`; a past-dated span still mints.
    #[test]
    fn mint_at_enforces_ttl_cap_and_ordering() {
        let kp = NodeKeypair::generate();
        let iat = 1_000_000;
        let over = mint_api_token_at(
            &kp,
            "VERSE#v1",
            "viewer",
            iat,
            iat + MAX_API_TOKEN_TTL_SECS + 1,
            "j1",
        );
        assert!(over.is_err(), "span over the cap must be refused");
        let backwards = mint_api_token_at(&kp, "VERSE#v1", "viewer", iat, iat - 1, "j2");
        assert!(backwards.is_err(), "exp before iat must be refused");
        let at_cap = mint_api_token_at(
            &kp,
            "VERSE#v1",
            "viewer",
            iat,
            iat + MAX_API_TOKEN_TTL_SECS,
            "j3",
        );
        assert!(at_cap.is_ok(), "a span exactly at the cap mints");
        // Expired-token test lane: a short span wholly in the past.
        assert!(mint_api_token_at(&kp, "VERSE#v1", "viewer", 100, 180, "j4").is_ok());
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

    /// TOKEN-CLIENT-CLAIM: a client token carries its DID through
    /// encode/verify with `sub` still the node; a legacy token decodes with no
    /// client and its encoded payload has no `client` key at all.
    #[test]
    fn a_client_token_round_trips_and_a_legacy_token_has_no_client() {
        let kp = NodeKeypair::generate();
        let client = new_client_did();
        assert!(validate_client_did(&client).is_ok());
        assert_ne!(client, kp.to_did_key(), "a fresh client is not the node");

        let token =
            mint_client_api_token(&kp, "VERSE#v1", "editor", 3600, "jti-c", &client).unwrap();
        let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
        assert_eq!(claims.client.as_deref(), Some(client.as_str()));
        assert_eq!(claims.sub, kp.to_did_key(), "sub stays the issuing node");
        assert_eq!(claims.max_role, "editor");

        let legacy = mint_api_token(&kp, "VERSE#v1", "editor", 3600, "jti-l").unwrap();
        let legacy_claims = verify_api_token(&legacy, &kp.verifying_key()).unwrap();
        assert_eq!(legacy_claims.client, None);
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.required_spec_claims = std::collections::HashSet::new();
        let raw = decode::<serde_json::Value>(
            &legacy,
            &DecodingKey::from_ed_der(&kp.verifying_key().to_bytes()),
            &validation,
        )
        .unwrap();
        let value = raw.claims;
        assert!(
            value.get("client").is_none(),
            "a legacy token's wire payload is unchanged (no client key)"
        );

        // Two fresh clients are distinct identities.
        assert_ne!(new_client_did(), new_client_did());
    }

    /// TOKEN-CLIENT-CLAIM grammar (threats TC-4/TC-5): a malformed client is
    /// refused at mint, and a correctly-signed token smuggling one is refused
    /// at verify.
    #[test]
    fn a_token_with_a_malformed_client_is_rejected() {
        let kp = NodeKeypair::generate();
        let x25519_did = crate::x25519::DeviceKeypair::generate().to_did_key();
        let oversized = format!("did:key:z{}", "6".repeat(MAX_CLIENT_DID_LEN));
        let cases: [(&str, &str); 4] = [
            ("not a DID", "alice"),
            ("x25519 did:key", x25519_did.as_str()),
            ("oversized", oversized.as_str()),
            ("empty", ""),
        ];
        for (why, client) in cases {
            assert!(validate_client_did(client).is_err(), "{why}: grammar");
            assert!(
                mint_client_api_token(&kp, "VERSE#v1", "viewer", 3600, "j", client).is_err(),
                "{why}: refused at mint"
            );
            // Signed by the real node key, bypassing the mint-side check.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let smuggled = mint_with_client(
                &kp,
                "VERSE#v1",
                "viewer",
                now,
                now + 3600,
                "j",
                Some(client.to_string()),
            )
            .unwrap();
            assert!(
                verify_api_token(&smuggled, &kp.verifying_key()).is_err(),
                "{why}: refused at verify"
            );
        }
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
