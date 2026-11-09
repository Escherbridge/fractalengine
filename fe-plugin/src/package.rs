//! Publisher-signed plugin package verification (rationale: src/AGENTS.md §signed-packages).

use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

use crate::trust_roots::TrustRoots;

/// Ceiling on `{key_id, signature}` entries, bounding verification work (package-format.md §Signatures).
pub const MAX_SIGNATURE_ENTRIES: usize = 16;

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
    #[error("package '{package_id}': the signed manifest names id {manifest_id:?}")]
    IdMismatch {
        package_id: String,
        manifest_id: Option<String>,
    },
    #[error("package '{package_id}' carries {count} signature entries, over the limit")]
    TooManySignatures { package_id: String, count: usize },
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

/// The `id` the signed manifest names; the only authenticated source of a package id.
pub fn manifest_package_id(manifest_bytes: &[u8]) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_slice(manifest_bytes).ok()?;
    manifest.get("id")?.as_str().map(str::to_string)
}

/// Accepts the package when its signed manifest names `package_id`, no trust-root entry fails and at least one verifies.
pub fn verify_package(
    package_id: &str,
    manifest_bytes: &[u8],
    payload: &[u8],
    signatures: &[SignatureEntry],
    trust_roots: &TrustRoots,
    policy: UnsignedPolicy,
) -> Result<Verified, PackageError> {
    let manifest_id = manifest_package_id(manifest_bytes);
    if manifest_id.as_deref() != Some(package_id) {
        return Err(PackageError::IdMismatch {
            package_id: package_id.to_string(),
            manifest_id,
        });
    }
    if signatures.len() > MAX_SIGNATURE_ENTRIES {
        return Err(PackageError::TooManySignatures {
            package_id: package_id.to_string(),
            count: signatures.len(),
        });
    }
    if signatures.is_empty() {
        return if policy.admits_unverified(package_id, "unsigned") {
            Ok(Verified::DevUnverified)
        } else {
            Err(PackageError::Unsigned(package_id.to_string()))
        };
    }
    let message = signed_message(manifest_bytes, payload);
    let mut verified_by: Option<&str> = None;
    for entry in signatures {
        let Some(key) = trust_roots.get(&entry.key_id) else {
            continue;
        };
        let valid = hex::decode(&entry.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .is_some_and(|signature| key.verifying_key.verify_strict(&message, &signature).is_ok());
        if !valid {
            // A failing trusted entry is tampering; another valid entry never outvotes it.
            return Err(PackageError::InvalidSignature {
                package_id: package_id.to_string(),
                key_id: entry.key_id.clone(),
            });
        }
        verified_by.get_or_insert(entry.key_id.as_str());
    }
    match verified_by {
        Some(key_id) => Ok(Verified::Signed {
            key_id: key_id.to_string(),
        }),
        None if policy.admits_unverified(package_id, "untrusted key") => Ok(Verified::DevUnverified),
        None => Err(PackageError::UntrustedKey {
            package_id: package_id.to_string(),
        }),
    }
}
