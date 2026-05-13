use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use fe_identity::did_key::public_key_from_did_key;

/// Sign a manifest JSON blob with the publisher's ed25519 key.
///
/// Returns a base64-encoded signature string.
pub fn sign_manifest(
    manifest_json: &[u8],
    keypair: &SigningKey,
) -> Result<String, anyhow::Error> {
    let sig: Signature = keypair.sign(manifest_json);
    Ok(STANDARD.encode(sig.to_bytes()))
}

/// Verify a manifest signature against a publisher's did:key.
///
/// Extracts the Ed25519 verifying key from the did:key string and checks
/// the base64-encoded signature against the manifest JSON bytes.
pub fn verify_manifest(
    manifest_json: &[u8],
    signature: &str,
    publisher_did: &str,
) -> Result<bool, anyhow::Error> {
    let verifying_key: VerifyingKey = public_key_from_did_key(publisher_did)?;

    let sig_bytes = STANDARD
        .decode(signature)
        .map_err(|e| anyhow::anyhow!("base64 decode failed: {}", e))?;

    let sig: Signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("invalid signature bytes: {}", e))?;

    // Use verify_strict to prevent small-subgroup attacks
    Ok(verifying_key.verify_strict(manifest_json, &sig).is_ok())
}

/// Compute a BLAKE3 hash of the manifest JSON for deduplication.
pub fn manifest_hash(manifest_json: &[u8]) -> String {
    let hash = blake3::hash(manifest_json);
    hash.to_hex().to_string()
}

/// Compute a BLAKE3 hash of arbitrary asset bytes.
pub fn asset_hash(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hash.to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn test_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let keypair = test_signing_key();
        let manifest = br#"{"name":"test","version":"1.0.0"}"#;
        let publisher_did = format!("did:key:z{}", {
            use multibase::Base;
            let pub_key = keypair.verifying_key();
            let mut bytes = vec![0xed, 0x01];
            bytes.extend_from_slice(&pub_key.to_bytes());
            multibase::encode(Base::Base58Btc, &bytes)
                .strip_prefix("z")
                .unwrap()
                .to_string()
        });

        let sig = sign_manifest(manifest, &keypair).expect("signing should succeed");
        let valid = verify_manifest(manifest, &sig, &publisher_did)
            .expect("verification should not error");
        assert!(valid, "signature should be valid");
    }

    #[test]
    fn test_tampered_manifest_fails() {
        let keypair = test_signing_key();
        let manifest = br#"{"name":"test","version":"1.0.0"}"#;
        let publisher_did = format!("did:key:z{}", {
            use multibase::Base;
            let pub_key = keypair.verifying_key();
            let mut bytes = vec![0xed, 0x01];
            bytes.extend_from_slice(&pub_key.to_bytes());
            multibase::encode(Base::Base58Btc, &bytes)
                .strip_prefix("z")
                .unwrap()
                .to_string()
        });

        let sig = sign_manifest(manifest, &keypair).expect("signing should succeed");
        let tampered = br#"{"name":"test","version":"2.0.0"}"#;
        let valid = verify_manifest(tampered, &sig, &publisher_did)
            .expect("verification should not error");
        assert!(!valid, "tampered manifest should fail verification");
    }

    #[test]
    fn test_wrong_publisher_fails() {
        let keypair = test_signing_key();
        let wrong_keypair = test_signing_key();
        let manifest = br#"{"name":"test"}"#;

        let wrong_did = format!("did:key:z{}", {
            use multibase::Base;
            let pub_key = wrong_keypair.verifying_key();
            let mut bytes = vec![0xed, 0x01];
            bytes.extend_from_slice(&pub_key.to_bytes());
            multibase::encode(Base::Base58Btc, &bytes)
                .strip_prefix("z")
                .unwrap()
                .to_string()
        });

        let sig = sign_manifest(manifest, &keypair).expect("signing should succeed");
        let valid = verify_manifest(manifest, &sig, &wrong_did)
            .expect("verification should not error");
        assert!(!valid, "wrong publisher should fail verification");
    }

    #[test]
    fn test_invalid_did_key_errors() {
        let result = verify_manifest(b"{}", "some_sig", "not-a-did-key");
        assert!(result.is_err(), "invalid did:key should error");
    }

    #[test]
    fn test_manifest_hash_deterministic() {
        let data = br#"{"test":true}"#;
        let h1 = manifest_hash(data);
        let h2 = manifest_hash(data);
        assert_eq!(h1, h2, "same input must produce same hash");
    }

    #[test]
    fn test_asset_hash_different_for_different_data() {
        let h1 = asset_hash(b"hello");
        let h2 = asset_hash(b"world");
        assert_ne!(h1, h2, "different input must produce different hash");
    }
}
