//! Trust roots (`trusted_publishers.toml`) and the signed revocation-list consumer (src/AGENTS.md §signed-packages).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Supported revocation-list wire version.
pub const REVOCATION_SCHEMA_VERSION: u32 = 1;

/// One trusted publisher key.
#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub verifying_key: VerifyingKey,
    /// Only revocation-capable keys may sign a revocation list.
    pub may_revoke: bool,
    /// Where a host pulls this key's revocation list from (transport is the caller's).
    pub revocation_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrustRootsError {
    #[error("trust roots I/O: {0}")]
    Io(String),
    #[error("trust roots parse: {0}")]
    Parse(String),
    #[error("trust root '{0}': public key must be 32 bytes of hex")]
    BadKey(String),
    #[error("duplicate trust root key id '{0}'")]
    Duplicate(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RevocationError {
    #[error("unsupported revocation schema_version {0}")]
    UnsupportedSchema(u32),
    #[error("revocation signer '{0}' is not a trust root")]
    UnknownSigner(String),
    #[error("revocation signer '{0}' is not revocation-capable")]
    NotRevocationCapable(String),
    #[error("revocation list signature does not verify")]
    BadSignature,
}

#[derive(Deserialize)]
struct TrustFile {
    #[serde(default)]
    publisher: Vec<TrustFileEntry>,
}

#[derive(Deserialize)]
struct TrustFileEntry {
    key_id: String,
    public_key: String,
    #[serde(default)]
    revocation: bool,
    revocation_url: Option<String>,
}

/// Configured allow-list of publisher keys, keyed by key id.
#[derive(Debug, Clone, Default)]
pub struct TrustRoots {
    keys: HashMap<String, TrustedKey>,
}

impl TrustRoots {
    /// Parse `[[publisher]]` tables: `key_id`, `public_key` (hex), `revocation` (default false), `revocation_url`.
    pub fn from_toml_str(text: &str) -> Result<Self, TrustRootsError> {
        let file: TrustFile =
            toml::from_str(text).map_err(|e| TrustRootsError::Parse(e.to_string()))?;
        let mut roots = Self::default();
        for entry in file.publisher {
            let bytes: [u8; 32] = hex::decode(&entry.public_key)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| TrustRootsError::BadKey(entry.key_id.clone()))?;
            let verifying_key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| TrustRootsError::BadKey(entry.key_id.clone()))?;
            if roots.keys.contains_key(&entry.key_id) {
                return Err(TrustRootsError::Duplicate(entry.key_id));
            }
            roots.keys.insert(
                entry.key_id,
                TrustedKey {
                    verifying_key,
                    may_revoke: entry.revocation,
                    revocation_url: entry.revocation_url,
                },
            );
        }
        Ok(roots)
    }

    /// Load the trust-roots file at startup; a missing file is an error, not an empty allow-list.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TrustRootsError> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| TrustRootsError::Io(e.to_string()))?;
        Self::from_toml_str(&text)
    }

    pub fn get(&self, key_id: &str) -> Option<&TrustedKey> {
        self.keys.get(key_id)
    }

    /// `(key_id, url)` for every revocation-capable key with a configured list URL.
    pub fn revocation_sources(&self) -> Vec<(&str, &str)> {
        self.keys
            .iter()
            .filter(|(_, k)| k.may_revoke)
            .filter_map(|(id, k)| k.revocation_url.as_deref().map(|u| (id.as_str(), u)))
            .collect()
    }

    /// Verify a revocation list against the revocation-capable roots and return its entries as a set.
    pub fn verify_revocation_list(
        &self,
        list: &RevocationList,
    ) -> Result<RevocationSet, RevocationError> {
        if list.schema_version != REVOCATION_SCHEMA_VERSION {
            return Err(RevocationError::UnsupportedSchema(list.schema_version));
        }
        let key = self
            .get(&list.signer_key_id)
            .ok_or_else(|| RevocationError::UnknownSigner(list.signer_key_id.clone()))?;
        if !key.may_revoke {
            return Err(RevocationError::NotRevocationCapable(
                list.signer_key_id.clone(),
            ));
        }
        let signature = hex::decode(&list.signature)
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok())
            .ok_or(RevocationError::BadSignature)?;
        key.verifying_key
            .verify(&list.signed_bytes(), &signature)
            .map_err(|_| RevocationError::BadSignature)?;
        Ok(RevocationSet::from_entries(&list.entries))
    }
}

/// One revoked `(package_id, content_hash)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationEntry {
    pub package_id: String,
    pub content_hash: String,
    pub reason: String,
    pub revoked_at: String,
}

/// Revocation list wire document, schema v1 (never changed in place).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationList {
    pub schema_version: u32,
    pub issued_at: String,
    pub entries: Vec<RevocationEntry>,
    pub signer_key_id: String,
    /// Hex ed25519 signature over [`RevocationList::signed_bytes`].
    pub signature: String,
}

impl RevocationList {
    /// Canonical (sorted-key, compact) JSON of `{entries, issued_at}`.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let value = serde_json::json!({ "entries": self.entries, "issued_at": self.issued_at });
        let mut out = String::new();
        write_canonical(&value, &mut out);
        out.into_bytes()
    }
}

/// Canonical JSON writer: object keys sorted, no whitespace.
fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Verified revoked pairs, queried at install and activation.
#[derive(Debug, Clone, Default)]
pub struct RevocationSet {
    pairs: HashSet<(String, String)>,
}

impl RevocationSet {
    pub fn from_entries(entries: &[RevocationEntry]) -> Self {
        Self {
            pairs: entries
                .iter()
                .map(|e| (e.package_id.clone(), e.content_hash.clone()))
                .collect(),
        }
    }

    pub fn is_revoked(&self, package_id: &str, content_hash: &str) -> bool {
        self.pairs
            .contains(&(package_id.to_string(), content_hash.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn revocation_list_honoured_only_from_revocation_capable_root() {
        let revoker = SigningKey::from_bytes(&[3u8; 32]);
        let publisher = SigningKey::from_bytes(&[4u8; 32]);
        let toml = format!(
            "[[publisher]]\nkey_id = \"rev\"\npublic_key = \"{}\"\nrevocation = true\nrevocation_url = \"https://example.org/revoked.json\"\n\n[[publisher]]\nkey_id = \"pub\"\npublic_key = \"{}\"\n",
            hex::encode(revoker.verifying_key().to_bytes()),
            hex::encode(publisher.verifying_key().to_bytes()),
        );
        let roots = TrustRoots::from_toml_str(&toml).unwrap();
        assert_eq!(roots.revocation_sources().len(), 1);

        let mut list = RevocationList {
            schema_version: 1,
            issued_at: "2026-11-08T00:00:00Z".into(),
            entries: vec![RevocationEntry {
                package_id: "p".into(),
                content_hash: "h".into(),
                reason: "malicious".into(),
                revoked_at: "2026-11-08T00:00:00Z".into(),
            }],
            signer_key_id: "rev".into(),
            signature: String::new(),
        };
        list.signature = hex::encode(revoker.sign(&list.signed_bytes()).to_bytes());
        assert!(roots.verify_revocation_list(&list).unwrap().is_revoked("p", "h"));

        // A publisher-only key can never revoke.
        list.signer_key_id = "pub".into();
        list.signature = hex::encode(publisher.sign(&list.signed_bytes()).to_bytes());
        assert_eq!(
            roots.verify_revocation_list(&list).unwrap_err(),
            RevocationError::NotRevocationCapable("pub".into())
        );
    }
}
