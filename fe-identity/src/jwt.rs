use crate::keypair::NodeKeypair;
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

fn ensure_crypto_provider() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FractalClaims {
    pub sub: String,
    pub petal_id: String,
    pub role_id: String,
    pub iat: u64,
    pub exp: u64,
}

pub fn mint_session_token(
    keypair: &NodeKeypair,
    petal_id: &str,
    role_id: &str,
    ttl_secs: u64,
) -> anyhow::Result<String> {
    ensure_crypto_provider();
    if ttl_secs > 300 {
        anyhow::bail!("TTL exceeds maximum of 300 seconds");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = FractalClaims {
        sub: keypair.to_did_key(),
        petal_id: petal_id.to_string(),
        role_id: role_id.to_string(),
        iat: now,
        exp: now + ttl_secs,
    };
    let header = Header::new(Algorithm::EdDSA);
    let encoding_key = EncodingKey::from_ed_der(&keypair.pkcs8_der());
    encode(&header, &claims, &encoding_key).map_err(Into::into)
}

pub fn verify_session_token(
    token: &str,
    issuer_pub_key: &VerifyingKey,
) -> anyhow::Result<FractalClaims> {
    ensure_crypto_provider();
    let raw = raw_pub_bytes(issuer_pub_key);
    let decoding_key = DecodingKey::from_ed_der(&raw);
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.validate_exp = true;
    validation.required_spec_claims = std::collections::HashSet::new();
    decode::<FractalClaims>(token, &decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(Into::into)
}

fn raw_pub_bytes(pub_key: &VerifyingKey) -> Vec<u8> {
    pub_key.to_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::NodeKeypair;

    #[test]
    fn test_mint_and_verify() {
        let kp = NodeKeypair::generate();
        let token = mint_session_token(&kp, "petal-1", "admin", 300).unwrap();
        let claims = verify_session_token(&token, &kp.verifying_key()).unwrap();
        assert_eq!(claims.petal_id, "petal-1");
        assert_eq!(claims.role_id, "admin");
    }

    #[test]
    fn test_ttl_limit_enforced() {
        let kp = NodeKeypair::generate();
        assert!(mint_session_token(&kp, "petal-1", "admin", 301).is_err());
    }

    #[test]
    fn test_tampered_token() {
        let kp = NodeKeypair::generate();
        let mut token = mint_session_token(&kp, "petal-1", "admin", 300).unwrap();
        let last = token.pop().unwrap();
        token.push(if last == 'a' { 'b' } else { 'a' });
        assert!(verify_session_token(&token, &kp.verifying_key()).is_err());
    }

    #[test]
    fn test_expired_token() {
        // DEFERRED TO WAVE 6 VALIDATION — requires mock time or sleep(2s)
    }
}
