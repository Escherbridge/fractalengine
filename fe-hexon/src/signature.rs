//! Hexon signing is fe-format's canonical-JSON scheme (see `src/AGENTS.md` §unified-format).

pub use fe_format::{sign_manifest, verify_manifest};

/// BLAKE3 hex of manifest JSON bytes, used as a dedupe key (not a signature input).
pub fn manifest_hash(manifest_json: &[u8]) -> String {
    blake3::hash(manifest_json).to_hex().to_string()
}

/// BLAKE3 hex of asset bytes: the `asset_hash` every entry and `assets/{hash}` member uses.
pub fn asset_hash(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use fe_format::{verifying_key_to_did, HexonManifest, HexonType};
    use rand::rngs::OsRng;

    fn manifest(publisher_did: &str) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "test-model".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: publisher_did.to_string(),
            publisher_name: None,
            version: "1.0.0".to_string(),
            build_id: None,
            name: "test".to_string(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }

    /// Sign a manifest struct, then check the signature against an edited or re-formatted copy.
    #[test]
    fn test_sign_and_verify_canonical_manifest() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = verifying_key_to_did(&keypair.verifying_key());
        let original = manifest(&did);
        let signature = sign_manifest(&serde_json::to_vec(&original).unwrap(), &keypair).unwrap();

        // The signature covers canonical JSON with `signature` blanked: pretty-printing the
        // signed manifest (signature filled in) still verifies.
        let mut signed = original.clone();
        signed.signature = Some(signature.clone());
        let pretty = serde_json::to_vec_pretty(&signed).unwrap();
        assert!(verify_manifest(&pretty, &signature, &did).unwrap());

        // One flipped field is refused.
        let mut tampered = signed.clone();
        tampered.version = "2.0.0".to_string();
        let tampered_json = serde_json::to_vec(&tampered).unwrap();
        assert!(!verify_manifest(&tampered_json, &signature, &did).unwrap(), "tampered manifest must fail");

        // Another publisher's DID is refused; a malformed DID is an error.
        let other = verifying_key_to_did(&SigningKey::generate(&mut OsRng).verifying_key());
        assert!(!verify_manifest(&pretty, &signature, &other).unwrap(), "wrong publisher must fail");
        assert!(verify_manifest(&pretty, &signature, "not-a-did-key").is_err());
    }
}
