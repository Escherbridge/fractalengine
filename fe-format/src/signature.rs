//! Ed25519 manifest signing and verification for the Hexon format.
//!
//! Implements `did:key:z6Mk...` public-key extraction, canonical JSON
//! serialisation, and sign / verify round-trips as defined in the Hexon v1.0.0
//! spec (`docs/hexon-format-spec.md`).

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::Value;

// ---- canonical JSON ---------------------------------------------------------

/// Produce canonical JSON: sorted keys recursively, no whitespace.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).expect("string serialisation cannot fail"),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .iter()
                .map(|k| {
                    let key_json =
                        serde_json::to_string(*k).expect("string serialisation cannot fail");
                    let val_json = canonical_json(map.get(*k).unwrap());
                    format!("{}:{}", key_json, val_json)
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
    }
}

// ---- signing ----------------------------------------------------------------

/// Sign a manifest JSON byte slice with an Ed25519 signing key.
/// Returns the base64-encoded signature string.
///
/// Process:
/// 1. Parse JSON, set "signature" field to ""
/// 2. Serialize to canonical JSON (sorted keys, no whitespace)
/// 3. Sign with [`ed25519_dalek::SigningKey`]
/// 4. Return base64 (standard encoding) of the 64-byte signature
pub fn sign_manifest(json: &[u8], key: &SigningKey) -> Result<String> {
    let mut doc: Value =
        serde_json::from_slice(json).context("sign_manifest: invalid JSON input")?;

    let obj = doc
        .as_object_mut()
        .context("sign_manifest: top-level value must be a JSON object")?;
    obj.insert("signature".to_string(), Value::String(String::new()));

    let canonical = canonical_json(&doc);
    let sig: Signature = key.sign(canonical.as_bytes());
    Ok(BASE64.encode(sig.to_bytes()))
}

// ---- verification -----------------------------------------------------------

/// Verify a manifest's signature against the publisher's DID.
///
/// Process:
/// 1. Parse JSON, extract "signature" field, set it to ""
/// 2. Serialize to canonical JSON (sorted keys, no whitespace)
/// 3. Extract public key from `publisher_did` (`did:key:z6Mk...` format)
/// 4. Verify Ed25519 signature
pub fn verify_manifest(json: &[u8], sig: &str, publisher_did: &str) -> Result<bool> {
    let mut doc: Value =
        serde_json::from_slice(json).context("verify_manifest: invalid JSON input")?;

    let obj = doc
        .as_object_mut()
        .context("verify_manifest: top-level value must be a JSON object")?;
    obj.insert("signature".to_string(), Value::String(String::new()));

    let canonical = canonical_json(&doc);

    let sig_bytes = BASE64
        .decode(sig)
        .context("verify_manifest: invalid base64 signature")?;
    let signature = Signature::from_slice(&sig_bytes)
        .context("verify_manifest: signature must be exactly 64 bytes")?;

    let verifying_key = did_key_to_verifying_key(publisher_did)?;

    match verifying_key.verify(canonical.as_bytes(), &signature) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

// ---- did:key helpers --------------------------------------------------------

/// Extract an Ed25519 [`VerifyingKey`] from a `did:key:z6Mk...` DID string.
///
/// The DID is multibase-encoded (base58btc, `z` prefix) with a 2-byte
/// multicodec prefix `[0xed, 0x01]` for Ed25519 public keys.
fn did_key_to_verifying_key(did: &str) -> Result<VerifyingKey> {
    let multibase = did
        .strip_prefix("did:key:")
        .context("DID must start with \"did:key:\"")?;

    let z_stripped = multibase
        .strip_prefix('z')
        .context("multibase value must use base58btc ('z' prefix)")?;

    let decoded = bs58::decode(z_stripped)
        .into_vec()
        .context("invalid base58btc encoding in DID")?;

    if decoded.len() < 2 {
        bail!("decoded DID key is too short (need at least 2-byte multicodec prefix)");
    }
    if decoded[0] != 0xed || decoded[1] != 0x01 {
        bail!(
            "unexpected multicodec prefix [{:#04x}, {:#04x}]; expected Ed25519 [0xed, 0x01]",
            decoded[0],
            decoded[1]
        );
    }

    let key_bytes = &decoded[2..];
    if key_bytes.len() != 32 {
        bail!(
            "Ed25519 public key must be 32 bytes, got {}",
            key_bytes.len()
        );
    }

    let key_array: [u8; 32] = key_bytes.try_into().unwrap();
    VerifyingKey::from_bytes(&key_array).context("invalid Ed25519 public key")
}

/// Encode an Ed25519 [`VerifyingKey`] as a `did:key:z6Mk...` DID string.
///
/// This is the inverse of [`did_key_to_verifying_key`] and is useful in tests
/// and when publishing manifests.
pub fn verifying_key_to_did(key: &VerifyingKey) -> String {
    let mut buf = Vec::with_capacity(34);
    buf.push(0xed);
    buf.push(0x01);
    buf.extend_from_slice(key.as_bytes());
    let encoded = bs58::encode(&buf).into_string();
    format!("did:key:z{encoded}")
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_canonical_json() {
        let input = r#"{"z":1,"a":{"c":true,"b":[3,2,1]},"m":"hello"}"#;
        let value: Value = serde_json::from_str(input).unwrap();
        let canon = canonical_json(&value);
        // Keys sorted at every level; arrays preserve order; compact.
        assert_eq!(canon, r#"{"a":{"b":[3,2,1],"c":true},"m":"hello","z":1}"#);
    }

    #[test]
    fn test_sign_verify_roundtrip() {
        let key = SigningKey::generate(&mut OsRng);
        let did = verifying_key_to_did(&key.verifying_key());

        let manifest = serde_json::json!({
            "hexon_version": "1.0.0",
            "id": "test-hexon",
            "publisher": did,
            "signature": ""
        });
        let json = serde_json::to_vec(&manifest).unwrap();

        let sig = sign_manifest(&json, &key).unwrap();
        let ok = verify_manifest(&json, &sig, &did).unwrap();
        assert!(ok, "signature must verify for the same key");
    }

    #[test]
    fn test_tampered_signature_fails() {
        let key = SigningKey::generate(&mut OsRng);
        let did = verifying_key_to_did(&key.verifying_key());

        let manifest = serde_json::json!({
            "hexon_version": "1.0.0",
            "id": "test-hexon",
            "publisher": did,
            "signature": ""
        });
        let json = serde_json::to_vec(&manifest).unwrap();

        let sig = sign_manifest(&json, &key).unwrap();

        // Tamper: flip one byte in the decoded signature, then re-encode.
        let mut sig_bytes = BASE64.decode(&sig).unwrap();
        sig_bytes[0] ^= 0xff;
        let tampered = BASE64.encode(&sig_bytes);

        let ok = verify_manifest(&json, &tampered, &did).unwrap();
        assert!(!ok, "tampered signature must not verify");
    }

    #[test]
    fn test_did_key_extraction() {
        let key = SigningKey::generate(&mut OsRng);
        let vk = key.verifying_key();
        let did = verifying_key_to_did(&vk);

        assert!(did.starts_with("did:key:z6Mk"));

        let extracted = did_key_to_verifying_key(&did).unwrap();
        assert_eq!(
            extracted.as_bytes(),
            vk.as_bytes(),
            "extracted key must match the original"
        );
    }
}
