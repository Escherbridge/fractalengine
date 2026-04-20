use ed25519_dalek::VerifyingKey;
use multibase::Base;

/// Convert an Ed25519 public key to did:key format.
///
/// Format: `did:key:z` + base58btc(0xed01 || raw_pub_key_bytes)
/// The multicodec prefix 0xed01 identifies Ed25519 public keys.
pub fn did_key_from_public_key(pub_key: &VerifyingKey) -> String {
    let pub_bytes = pub_key.to_bytes();
    let mut multicodec = Vec::with_capacity(34);
    multicodec.push(0xed);
    multicodec.push(0x01);
    multicodec.extend_from_slice(&pub_bytes);
    let encoded = multibase::encode(Base::Base58Btc, &multicodec);
    format!("did:key:{}", encoded)
}

/// Backward-compat alias for [`did_key_from_public_key`].
#[inline]
pub fn pub_key_to_did_key(pub_key: &VerifyingKey) -> String {
    did_key_from_public_key(pub_key)
}

/// Parse a did:key string back to an Ed25519 VerifyingKey.
///
/// Expects `did:key:z<base58btc>` where the decoded bytes start with the
/// multicodec prefix `0xed 0x01` followed by 32 raw key bytes.
pub fn public_key_from_did_key(did: &str) -> anyhow::Result<VerifyingKey> {
    let encoded = did
        .strip_prefix("did:key:")
        .ok_or_else(|| anyhow::anyhow!("invalid did:key: must start with 'did:key:'"  ))?;

    let (base, bytes) = multibase::decode(encoded)
        .map_err(|e| anyhow::anyhow!("multibase decode failed: {}", e))?;

    if base != Base::Base58Btc {
        anyhow::bail!("did:key must use base58btc encoding (z prefix)");
    }

    if bytes.len() < 2 || bytes[0] != 0xed || bytes[1] != 0x01 {
        anyhow::bail!("invalid multicodec prefix: expected 0xed 0x01 for Ed25519");
    }

    let key_bytes: [u8; 32] = bytes[2..]
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid key length: expected 32 bytes, got {}", bytes.len() - 2))?;

    Ok(VerifyingKey::from_bytes(&key_bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::NodeKeypair;

    #[test]
    fn test_did_key_format() {
        let kp = NodeKeypair::generate();
        let did = did_key_from_public_key(&kp.verifying_key());
        assert!(did.starts_with("did:key:z6Mk"), "got: {}", did);
    }

    #[test]
    fn test_round_trip() {
        let kp = NodeKeypair::generate();
        let pub_key = kp.verifying_key();
        let did = did_key_from_public_key(&pub_key);
        let recovered = public_key_from_did_key(&did).expect("round-trip should succeed");
        assert_eq!(
            pub_key.to_bytes(),
            recovered.to_bytes(),
            "recovered key must match original"
        );
    }

    #[test]
    fn test_alias_matches_canonical() {
        let kp = NodeKeypair::generate();
        let vk = kp.verifying_key();
        assert_eq!(did_key_from_public_key(&vk), pub_key_to_did_key(&vk));
    }

    #[test]
    fn test_invalid_prefix_returns_err() {
        let result = public_key_from_did_key("did:web:example.com");
        assert!(result.is_err(), "non did:key should fail");
    }

    #[test]
    fn test_truncated_string_returns_err() {
        let result = public_key_from_did_key("did:key:z6Mk");
        assert!(result.is_err(), "truncated did:key should fail");
    }

    #[test]
    fn test_not_a_did_key_returns_err() {
        let result = public_key_from_did_key("not-a-did-key");
        assert!(result.is_err(), "random string should fail");
    }

    #[test]
    fn test_wrong_multicodec_prefix_returns_err() {
        // Build a did:key with a fake 0xff 0x01 prefix (not Ed25519)
        let mut fake_bytes = vec![0xff_u8, 0x01];
        fake_bytes.extend_from_slice(&[0u8; 32]);
        let encoded = multibase::encode(Base::Base58Btc, &fake_bytes);
        let did = format!("did:key:{}", encoded);
        let result = public_key_from_did_key(&did);
        assert!(result.is_err(), "wrong multicodec prefix should fail");
    }
}
