//! Trust roots (`trusted_publishers.toml`) and the signed revocation-list consumer (src/AGENTS.md §signed-packages).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::lifecycle::TrustTier;

/// Supported revocation-list wire version.
pub const REVOCATION_SCHEMA_VERSION: u32 = 1;

/// A trust root's declared role (governance.md Trust roots); absent in the file means `Publisher`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustRole {
    Release,
    #[default]
    Publisher,
    Dev,
}

impl TrustRole {
    /// The tier a package verified by a root of this role is trusted at.
    pub fn tier(self) -> TrustTier {
        match self {
            Self::Release => TrustTier::Release,
            Self::Publisher => TrustTier::Publisher,
            Self::Dev => TrustTier::Dev,
        }
    }
}

/// One trusted publisher key.
#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub verifying_key: VerifyingKey,
    /// Only revocation-capable keys may sign a revocation list.
    pub may_revoke: bool,
    /// Where a host pulls this key's revocation list from (transport is the caller's).
    pub revocation_url: Option<String>,
    pub role: TrustRole,
}

impl TrustedKey {
    /// The trust tier of packages this key verifies.
    pub fn tier(&self) -> TrustTier {
        self.role.tier()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrustRootsError {
    #[error("trust roots I/O: {0}")]
    Io(String),
    #[error("trust roots parse: {0}")]
    Parse(String),
    #[error("trust root '{0}': public key must be 32 bytes of hex encoding a full-order ed25519 point")]
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
    #[error("revocation list from '{signer}' issued at {issued_at} is older than the accepted {accepted}")]
    Stale { signer: String, issued_at: String, accepted: String },
    #[error("revocation list issued_at '{0}' is not RFC 3339 UTC (YYYY-MM-DDTHH:MM:SS[.fraction]Z)")]
    BadIssuedAt(String),
}

/// A revocation list's `issued_at` by value: `([year, month, day, hour, minute, second], nanos)`.
pub type IssuedInstant = ([u32; 6], u32);

/// Strict RFC 3339 UTC (`Z` only, up to 9 fraction digits) parsed for ordering; offsets and other shapes are `None`.
pub fn issued_at_instant(text: &str) -> Option<IssuedInstant> {
    let body = text.strip_suffix('Z')?;
    let (main, fraction) = body.split_once('.').unwrap_or((body, ""));
    let bytes = main.as_bytes();
    if bytes.len() != 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let field = |from: usize, to: usize| {
        main.get(from..to)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u32>().ok())
    };
    let stamp = [field(0, 4)?, field(5, 7)?, field(8, 10)?, field(11, 13)?, field(14, 16)?, field(17, 19)?];
    if (body.contains('.') && fraction.is_empty()) || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let nanos = format!("{fraction:0<9}").parse::<u32>().ok()?;
    Some((stamp, nanos))
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
    /// Absent = publisher; an unknown value fails the whole file (parse error).
    #[serde(default)]
    role: TrustRole,
}

/// Configured allow-list of publisher keys, keyed by key id.
#[derive(Debug, Clone, Default)]
pub struct TrustRoots {
    keys: HashMap<String, TrustedKey>,
}

impl TrustRoots {
    /// Parse `[[publisher]]` tables: `key_id`, `public_key` (hex), `revocation` (default false), `revocation_url`.
    pub fn from_toml_str(text: &str) -> Result<Self, TrustRootsError> {
        Self::from_toml_str_with(text, false)
    }

    /// As [`Self::from_toml_str`]; `dev` roots load only when `dev_flag` is set AND this is a debug build.
    pub fn from_toml_str_with(text: &str, dev_flag: bool) -> Result<Self, TrustRootsError> {
        let allow_dev = cfg!(debug_assertions) && dev_flag;
        let file: TrustFile =
            toml::from_str(text).map_err(|e| TrustRootsError::Parse(e.to_string()))?;
        let mut roots = Self::default();
        for entry in file.publisher {
            let bytes: [u8; 32] = hex::decode(&entry.public_key)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| TrustRootsError::BadKey(entry.key_id.clone()))?;
            // A weak (small-order) key would accept forged signatures; refuse it at load.
            let verifying_key = VerifyingKey::from_bytes(&bytes)
                .ok()
                .filter(|key| !key.is_weak())
                .ok_or_else(|| TrustRootsError::BadKey(entry.key_id.clone()))?;
            if roots.keys.contains_key(&entry.key_id) {
                return Err(TrustRootsError::Duplicate(entry.key_id));
            }
            if entry.role == TrustRole::Dev && !allow_dev {
                continue; // dev roots never load in release builds or without the flag
            }
            roots.keys.insert(
                entry.key_id,
                TrustedKey {
                    verifying_key,
                    may_revoke: entry.revocation,
                    revocation_url: entry.revocation_url,
                    role: entry.role,
                },
            );
        }
        Ok(roots)
    }

    /// Load the trust-roots file at startup; a missing file is an error, not an empty allow-list.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TrustRootsError> {
        Self::load_with(path, false)
    }

    /// [`Self::load`] with the debug-only dev flag.
    pub fn load_with(path: impl AsRef<Path>, dev_flag: bool) -> Result<Self, TrustRootsError> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| TrustRootsError::Io(e.to_string()))?;
        Self::from_toml_str_with(&text, dev_flag)
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

    /// Verify a list, then refuse it when older than the newest accepted for its signer; `newest` is that per-signer memory.
    pub fn verify_revocation_list_fresh(
        &self,
        list: &RevocationList,
        newest: &mut HashMap<String, String>,
    ) -> Result<RevocationSet, RevocationError> {
        let set = self.verify_revocation_list(list)?;
        // Compared by value, never as text: an offset form (`+05:00`) or a fraction would order wrongly and
        // could pass an older list as newer. Only a verified list may move the high-water mark.
        let issued = issued_at_instant(&list.issued_at)
            .ok_or_else(|| RevocationError::BadIssuedAt(list.issued_at.clone()))?;
        if let Some(accepted) = newest.get(&list.signer_key_id) {
            // Only parsed marks are ever stored here; a foreign unparseable one never blocks a newer revocation.
            if issued_at_instant(accepted).is_some_and(|mark| issued < mark) {
                return Err(RevocationError::Stale {
                    signer: list.signer_key_id.clone(),
                    issued_at: list.issued_at.clone(),
                    accepted: accepted.clone(),
                });
            }
        }
        newest.insert(list.signer_key_id.clone(), list.issued_at.clone());
        Ok(set)
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
            .verify_strict(&list.signed_bytes(), &signature)
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

/// Revoked content hashes from verified lists, queried at install and activation.
#[derive(Debug, Clone, Default)]
pub struct RevocationSet {
    hashes: HashSet<String>,
}

impl RevocationSet {
    pub fn from_entries(entries: &[RevocationEntry]) -> Self {
        Self {
            hashes: entries
                .iter()
                .map(|e| e.content_hash.to_ascii_lowercase())
                .collect(),
        }
    }

    /// Union another verified list in (one list per revocation-capable root).
    pub fn extend(&mut self, other: RevocationSet) {
        self.hashes.extend(other.hashes);
    }

    /// True when any accepted entry names this content hash, whatever package id it names (renamed copies stay revoked).
    pub fn is_revoked(&self, content_hash: &str) -> bool {
        self.hashes.contains(&content_hash.to_ascii_lowercase())
    }

    /// True when any of these digests (content_hash, component_digest, plaintext_digest) is listed, under any package id.
    pub fn is_revoked_any(&self, digests: &[&str]) -> bool {
        digests.iter().any(|digest| self.is_revoked(digest))
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
        let revoked = roots.verify_revocation_list(&list).unwrap();
        assert!(revoked.is_revoked("h"), "the listed build is revoked");
        assert!(revoked.is_revoked("H"), "hash case never dodges the list");
        assert!(!revoked.is_revoked("other"));

        // An edited list no longer verifies against the revoker's signature.
        list.entries[0].content_hash = "x".into();
        assert_eq!(roots.verify_revocation_list(&list).unwrap_err(), RevocationError::BadSignature);

        // A publisher-only key can never revoke.
        list.signer_key_id = "pub".into();
        list.signature = hex::encode(publisher.sign(&list.signed_bytes()).to_bytes());
        assert_eq!(
            roots.verify_revocation_list(&list).unwrap_err(),
            RevocationError::NotRevocationCapable("pub".into())
        );

        // A weak (small-order) key never loads as a trust root.
        let identity_point = format!("01{}", "00".repeat(31));
        let weak = format!("[[publisher]]\nkey_id = \"weak\"\npublic_key = \"{identity_point}\"\n");
        assert!(matches!(TrustRoots::from_toml_str(&weak), Err(TrustRootsError::BadKey(_))));
    }

    fn key_hex(seed: u8) -> String {
        hex::encode(SigningKey::from_bytes(&[seed; 32]).verifying_key().to_bytes())
    }

    #[test]
    fn roles_map_to_tiers_and_dev_roots_need_the_debug_flag() {
        let toml = format!(
            "[[publisher]]\nkey_id = \"a\"\npublic_key = \"{}\"\nrole = \"release\"\n\n[[publisher]]\nkey_id = \"b\"\npublic_key = \"{}\"\n\n[[publisher]]\nkey_id = \"c\"\npublic_key = \"{}\"\nrole = \"dev\"\n",
            key_hex(5),
            key_hex(6),
            key_hex(7)
        );
        let roots = TrustRoots::from_toml_str(&toml).unwrap();
        assert_eq!(roots.get("a").unwrap().tier(), TrustTier::Release);
        assert_eq!(roots.get("b").unwrap().tier(), TrustTier::Publisher, "absent role = publisher");
        assert!(roots.get("c").is_none(), "dev root stays out without the flag");
        let with_flag = TrustRoots::from_toml_str_with(&toml, true).unwrap();
        assert_eq!(with_flag.get("c").is_some(), cfg!(debug_assertions), "flag counts in debug builds only");
        if cfg!(debug_assertions) {
            assert_eq!(with_flag.get("c").unwrap().tier(), TrustTier::Dev);
        }
        let unknown = format!("[[publisher]]\nkey_id = \"x\"\npublic_key = \"{}\"\nrole = \"root\"\n", key_hex(8));
        assert!(matches!(TrustRoots::from_toml_str(&unknown), Err(TrustRootsError::Parse(_))));
    }

    #[test]
    fn any_digest_revokes_and_stale_lists_are_refused() {
        let revoker = SigningKey::from_bytes(&[9u8; 32]);
        let toml = format!(
            "[[publisher]]\nkey_id = \"rev\"\npublic_key = \"{}\"\nrevocation = true\n",
            hex::encode(revoker.verifying_key().to_bytes())
        );
        let roots = TrustRoots::from_toml_str(&toml).unwrap();
        let signed = |issued_at: &str, hash: &str| {
            let mut list = RevocationList {
                schema_version: 1,
                issued_at: issued_at.into(),
                entries: vec![RevocationEntry {
                    package_id: "p".into(),
                    content_hash: hash.into(),
                    reason: "r".into(),
                    revoked_at: issued_at.into(),
                }],
                signer_key_id: "rev".into(),
                signature: String::new(),
            };
            list.signature = hex::encode(revoker.sign(&list.signed_bytes()).to_bytes());
            list
        };
        let mut newest = HashMap::new();
        let set = roots
            .verify_revocation_list_fresh(&signed("2026-11-08T00:00:00Z", "comp"), &mut newest)
            .unwrap();
        assert!(set.is_revoked_any(&["manifest-hash", "COMP"]), "component digest alone revokes the build");
        assert!(!set.is_revoked_any(&["manifest-hash", "other"]));
        assert!(!set.is_revoked_any(&[]));

        // Replaying an older (pre-revocation) list is refused; a newer one is accepted.
        let err = roots
            .verify_revocation_list_fresh(&signed("2026-11-01T00:00:00Z", "x"), &mut newest)
            .unwrap_err();
        assert!(matches!(err, RevocationError::Stale { .. }));
        roots
            .verify_revocation_list_fresh(&signed("2026-11-09T00:00:00Z", "x"), &mut newest)
            .unwrap();
        assert_eq!(newest["rev"], "2026-11-09T00:00:00Z");

        // Ordering is by value: a fraction that sorts below `Z` as text is still newer; an offset form whose text
        // sorts above the mark (an older instant) never reaches the comparison.
        roots
            .verify_revocation_list_fresh(&signed("2026-11-09T00:00:00.5Z", "y"), &mut newest)
            .expect("half a second later is newer");
        let offset = roots
            .verify_revocation_list_fresh(&signed("2026-11-09T23:00:00+05:00", "y"), &mut newest)
            .unwrap_err();
        assert!(matches!(offset, RevocationError::BadIssuedAt(_)));
        assert_eq!(newest["rev"], "2026-11-09T00:00:00.5Z");
    }
}
