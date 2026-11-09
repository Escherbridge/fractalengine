//! Publisher-signed plugin package verification (rationale: src/AGENTS.md §signed-packages).

use ed25519_dalek::{Signature, Verifier};
use serde::{Deserialize, Serialize};

use crate::trust_roots::TrustRoots;

/// One `{key_id, signature}` entry; `signature` is hex of the 64-byte ed25519 signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEntry {
    pub key_id: String,
    pub signature: String,
}

/// Whether a package without a verifiable trusted signature may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnsignedPolicy {
    /// Unsigned or untrusted-key packages are refused (the only policy in release builds).
    #[default]
    Refuse,
    /// Explicit developer escape hatch; compiled out of release builds.
    #[cfg(debug_assertions)]
    AllowDev,
}

impl UnsignedPolicy {
    /// True when the dev flag admits an unverifiable package; logs loudly every time.
    fn admits_unverified(&self, package_id: &str, reason: &str) -> bool {
        #[cfg(debug_assertions)]
        {
            if matches!(self, Self::AllowDev) {
                tracing::warn!(plugin = %package_id, %reason, "DEV FLAG: installing a plugin package without a trusted signature");
                return true;
            }
        }
        let _ = (self, package_id, reason);
        false
    }
}

/// Typed package verification failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    #[error("package '{0}' carries no signature")]
    Unsigned(String),
    #[error("package '{package_id}' is signed only by keys outside the trust roots")]
    UntrustedKey { package_id: String },
    #[error("package '{package_id}': signature by trusted key '{key_id}' does not verify")]
    InvalidSignature { package_id: String, key_id: String },
    #[error("package '{package_id}' is revoked (content hash {content_hash})")]
    Revoked {
        package_id: String,
        content_hash: String,
    },
    #[error("malformed signature entry for key '{0}'")]
    MalformedSignature(String),
}

/// How a package was admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// A trust-root key verified the signature.
    Signed { key_id: String },
    /// Admitted by the dev flag without a trusted signature.
    DevUnverified,
}

/// Hex blake3 of the payload; this is the content hash signed and revoked by.
pub fn payload_hash(payload: &[u8]) -> String {
    blake3::hash(payload).to_hex().to_string()
}

/// The signed message: `manifest_bytes || blake3(payload)` (32 raw digest bytes).
pub fn signed_message(manifest_bytes: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(manifest_bytes.len() + 32);
    message.extend_from_slice(manifest_bytes);
    message.extend_from_slice(blake3::hash(payload).as_bytes());
    message
}

/// Accepts the package when at least one entry verifies against a trust root.
pub fn verify_package(
    package_id: &str,
    manifest_bytes: &[u8],
    payload: &[u8],
    signatures: &[SignatureEntry],
    trust_roots: &TrustRoots,
    policy: UnsignedPolicy,
) -> Result<Verified, PackageError> {
    if signatures.is_empty() {
        return if policy.admits_unverified(package_id, "unsigned") {
            Ok(Verified::DevUnverified)
        } else {
            Err(PackageError::Unsigned(package_id.to_string()))
        };
    }
    let message = signed_message(manifest_bytes, payload);
    let mut failed_trusted_key: Option<String> = None;
    for entry in signatures {
        let Some(key) = trust_roots.get(&entry.key_id) else {
            continue;
        };
        let signature = hex::decode(&entry.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .ok_or_else(|| PackageError::MalformedSignature(entry.key_id.clone()))?;
        if key.verifying_key.verify(&message, &signature).is_ok() {
            return Ok(Verified::Signed {
                key_id: entry.key_id.clone(),
            });
        }
        failed_trusted_key.get_or_insert_with(|| entry.key_id.clone());
    }
    match failed_trusted_key {
        Some(key_id) => Err(PackageError::InvalidSignature {
            package_id: package_id.to_string(),
            key_id,
        }),
        None if policy.admits_unverified(package_id, "untrusted key") => Ok(Verified::DevUnverified),
        None => Err(PackageError::UntrustedKey {
            package_id: package_id.to_string(),
        }),
    }
}
