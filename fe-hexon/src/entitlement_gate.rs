//! FR-6 client-side enforcement: a licensed package loads only its `free_entries` without a valid
//! token; with one, sealed entries open in memory on every load and plaintext is never persisted
//! (see `fe-hexon/src/AGENTS.md` §entitlement).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Cursor;

use fe_format::entitlement::{
    open_paid_entry, unix_now, ContentKey, EntitlementClaims, EntitlementError, EntitlementExpectation,
    EntitlementVerifier, TOKEN_CLOCK_LEEWAY_SECS,
};
use fe_format::limits::{read_entry_capped, MAX_JSON_ENTRY_BYTES};

use crate::entitlement_client::Licensee;
use crate::manifest::{License, LicenseType};
use crate::package::HexonPackageData;

/// What `license.json` says about entitlement: whether one is needed and which entries are free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LicenseTerms {
    licensed: bool,
    free_entries: BTreeSet<String>,
}

impl LicenseTerms {
    /// No `license.json` (pre-entitlement archives): everything loads as before.
    pub fn unlicensed() -> Self {
        Self::default()
    }

    /// Terms of a hexon package licence: only `Paid` requires an entitlement.
    pub fn from_license(license: &License) -> Self {
        Self {
            licensed: license.license_type == LicenseType::Paid,
            free_entries: license.free_entries.iter().cloned().collect(),
        }
    }

    /// Terms of an fe-format (sealed `.hexon`) licence.
    pub fn from_format_license(license: &fe_format::License) -> Self {
        Self {
            licensed: matches!(license.license_type, fe_format::LicenseType::Paid),
            free_entries: license.free_entries.iter().cloned().collect(),
        }
    }

    /// True when non-free entries need an entitlement.
    pub fn is_licensed(&self) -> bool {
        self.licensed
    }

    /// True when `entry_id` loads without a token.
    pub fn is_free_entry(&self, entry_id: &str) -> bool {
        self.free_entries.contains(entry_id)
    }
}

/// Why a licensed package is limited to its free entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewReason {
    /// No token is held.
    NoToken,
    /// The held token was refused (forged kid, wrong build, expired, wrong device, ...).
    Refused(EntitlementError),
}

/// The inspectable entitlement state of an opened package (Hexon Manager FR-7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateStatus {
    /// Free or attribution licence, or no licence: the verifier is never consulted.
    NotRequired,
    /// Licensed, no valid token: only free entries load.
    Preview(PreviewReason),
    /// Licensed with a verified token, valid until `exp`.
    Entitled { exp: u64 },
}

/// Why an entry did not load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateRefusal {
    /// A non-free entry of a licensed package without a valid token (a state, not a fault).
    #[error("entry `{entry_id}` needs an entitlement")]
    EntitlementRequired { entry_id: String },
    /// No entry with this id in the package.
    #[error("no entry `{0}` in this package")]
    UnknownEntry(String),
    /// The entry's content member is missing.
    #[error("entry `{0}` has no content in the archive")]
    MissingContent(String),
    /// The sealed bytes do not open (tampered, swapped or another build): never a plaintext fallback.
    #[error("entry `{entry_id}` does not open: {error}")]
    OpenFailed { entry_id: String, error: EntitlementError },
}

/// Access decided at open time.
enum Access {
    NotRequired,
    Preview(PreviewReason),
    Entitled { claims: EntitlementClaims, content_key: ContentKey },
}

/// Gate over one opened package build; holds the content key (zeroized on drop), never plaintext.
pub struct EntitlementGate {
    terms: LicenseTerms,
    access: Access,
}

impl std::fmt::Debug for EntitlementGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("EntitlementGate").field("terms", &self.terms).field("status", &self.status()).finish()
    }
}

impl EntitlementGate {
    /// Decide access for one build: an unlicensed package never consults the verifier (NFR-2);
    /// a licensed one is entitled only when `token` verifies for this package, build, licensee and device.
    pub fn open(
        terms: LicenseTerms,
        package_id: &str,
        content_hash: &str,
        licensee: &Licensee<'_>,
        token: Option<&str>,
        verifier: &dyn EntitlementVerifier,
        now: u64,
    ) -> Self {
        let access = if !terms.is_licensed() {
            Access::NotRequired
        } else if let Some(token) = token {
            let licensee_did = licensee.did();
            let expected = EntitlementExpectation { package_id, content_hash, licensee_did: &licensee_did };
            match verifier.verify(token, &expected, licensee.device_secret, now) {
                Ok(verified) => Access::Entitled { claims: verified.claims, content_key: verified.content_key },
                Err(error) => {
                    tracing::info!(package_id, %error, "entitlement token refused; free entries only");
                    Access::Preview(PreviewReason::Refused(error))
                }
            }
        } else {
            Access::Preview(PreviewReason::NoToken)
        };
        Self { terms, access }
    }

    /// The licence terms this gate enforces.
    pub fn terms(&self) -> &LicenseTerms {
        &self.terms
    }

    /// Status as decided at open time.
    pub fn status(&self) -> GateStatus {
        match &self.access {
            Access::NotRequired => GateStatus::NotRequired,
            Access::Preview(reason) => GateStatus::Preview(reason.clone()),
            Access::Entitled { claims, .. } => GateStatus::Entitled { exp: claims.exp },
        }
    }

    /// Status at `now`: an entitlement whose token has expired reads as a refused preview.
    pub fn status_at(&self, now: u64) -> GateStatus {
        match &self.access {
            Access::Entitled { claims, .. } if Self::expired(claims, now) => {
                GateStatus::Preview(PreviewReason::Refused(EntitlementError::Expired { exp: claims.exp, now }))
            }
            _ => self.status(),
        }
    }

    /// Hexon Manager row label: `installed`, `installed-preview`, `entitlement-required` or `entitled`.
    pub fn status_label_at(&self, now: u64) -> &'static str {
        match self.status_at(now) {
            GateStatus::NotRequired => "installed",
            GateStatus::Entitled { .. } => "entitled",
            GateStatus::Preview(_) if !self.terms.free_entries.is_empty() => "installed-preview",
            GateStatus::Preview(_) => "entitlement-required",
        }
    }

    fn expired(claims: &EntitlementClaims, now: u64) -> bool {
        now > claims.exp.saturating_add(TOKEN_CLOCK_LEEWAY_SECS)
    }

    /// Load one entry's stored bytes now; see [`load_at`](Self::load_at).
    pub fn load<'bytes>(&self, entry_id: &str, stored: &'bytes [u8]) -> Result<Cow<'bytes, [u8]>, GateRefusal> {
        self.load_at(entry_id, stored, unix_now())
    }

    /// Unlicensed and free entries pass through untouched; a non-free entry opens in memory only
    /// while the token is unexpired, and the caller must not write the returned plaintext to disk.
    pub fn load_at<'bytes>(&self, entry_id: &str, stored: &'bytes [u8], now: u64) -> Result<Cow<'bytes, [u8]>, GateRefusal> {
        match &self.access {
            Access::NotRequired => Ok(Cow::Borrowed(stored)),
            _ if self.terms.is_free_entry(entry_id) => Ok(Cow::Borrowed(stored)),
            Access::Entitled { claims, content_key } if !Self::expired(claims, now) => {
                open_paid_entry(content_key, &claims.package_id, entry_id, stored)
                    .map(Cow::Owned)
                    .map_err(|error| GateRefusal::OpenFailed { entry_id: entry_id.to_owned(), error })
            }
            _ => Err(GateRefusal::EntitlementRequired { entry_id: entry_id.to_owned() }),
        }
    }

    /// Load an entry of an opened hexon package (content looked up by the entry's `asset_hash`).
    pub fn load_package_entry<'package>(
        &self,
        package: &'package HexonPackageData,
        entry_id: &str,
    ) -> Result<Cow<'package, [u8]>, GateRefusal> {
        let entry = package
            .entries
            .iter()
            .find(|entry| entry.entry_id == entry_id)
            .ok_or_else(|| GateRefusal::UnknownEntry(entry_id.to_owned()))?;
        let stored = package
            .assets
            .get(&entry.asset_hash)
            .ok_or_else(|| GateRefusal::MissingContent(entry_id.to_owned()))?;
        self.load(entry_id, stored)
    }
}

/// The archive's `content_hash`, recomputed from its own `manifest.json` (never from a document).
pub fn archive_content_hash(archive: &[u8]) -> anyhow::Result<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    let mut manifest = zip.by_name("manifest.json")?;
    let declared = manifest.size();
    let mut budget = MAX_JSON_ENTRY_BYTES;
    let raw = read_entry_capped(&mut manifest, declared, MAX_JSON_ENTRY_BYTES, &mut budget)?;
    fe_format::integrity::content_hash(&raw)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ed25519_dalek::SigningKey;
    use fe_format::entitlement::{seal_paid_entry, KeyringVerifier, VerifiedEntitlement};

    use super::*;
    use crate::entitlement_client::test_endpoint::FakeEndpoint;
    use crate::entitlement_client::{EntitlementClient, MemoryTokenStore, TokenTarget};
    use crate::registry_source::{CatalogConfig, FEATURE_ENTITLEMENTS};

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    const NOW: u64 = 1_762_473_600;
    const PACKAGE: &str = "org.example.analytics-pack";

    fn random_bytes() -> [u8; 32] {
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
        bytes
    }

    /// A verifier that must never be reached (NFR-2: unlicensed packages skip verification).
    struct UnreachableVerifier;

    impl EntitlementVerifier for UnreachableVerifier {
        fn verify(&self, _: &str, _: &EntitlementExpectation<'_>, _: &[u8; 32], _: u64) -> Result<VerifiedEntitlement, EntitlementError> {
            panic!("an unlicensed package consulted the entitlement verifier");
        }
    }

    /// AC-1..AC-4 journey: unlicensed loads as before; a paid archive with no token loads only
    /// `preview_1`; a token from the endpoint opens every sealed entry to its exact plaintext; a
    /// token for another build, another device or past exp is refused, never plaintext.
    #[test]
    fn licensed_package_loads_free_entries_without_a_token_and_opens_sealed_ones_only_when_entitled() {
        let content_key = ContentKey::generate().unwrap();
        let sealed = seal_paid_entry(&content_key, PACKAGE, "terrain_full", b"full-resolution tiles").unwrap();
        let preview = b"low-resolution preview".to_vec();
        let identity = SigningKey::from_bytes(&random_bytes());
        let device = random_bytes();
        let licensee = Licensee { identity: &identity, device_secret: &device };
        let terms = LicenseTerms::from_license(&License {
            license_type: LicenseType::Paid,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec!["preview_1".into()],
            encrypted_key: None,
        });

        // AC-1: an unlicensed archive never reaches the verifier and loads every entry verbatim.
        let open = EntitlementGate::open(LicenseTerms::unlicensed(), PACKAGE, HASH, &licensee, Some("junk"), &UnreachableVerifier, NOW);
        assert_eq!((open.status(), open.status_label_at(NOW)), (GateStatus::NotRequired, "installed"));
        assert_eq!(open.load_at("terrain_full", &sealed, NOW).unwrap(), Cow::Borrowed(&sealed[..]));

        // AC-2: no token → preview; the free entry loads, the sealed one is a distinct refusal.
        let endpoint = FakeEndpoint::new("issuer-1", PACKAGE, &content_key, NOW);
        let verifier: Arc<KeyringVerifier> = endpoint.verifier();
        let preview_gate = EntitlementGate::open(terms.clone(), PACKAGE, HASH, &licensee, None, verifier.as_ref(), NOW);
        assert_eq!(preview_gate.status_label_at(NOW), "installed-preview");
        assert_eq!(preview_gate.load_at("preview_1", &preview, NOW).unwrap(), Cow::Borrowed(&preview[..]));
        assert_eq!(
            preview_gate.load_at("terrain_full", &sealed, NOW),
            Err(GateRefusal::EntitlementRequired { entry_id: "terrain_full".into() })
        );

        // AC-3: the token client fetches a token; the gate recovers the key and opens the entry exactly.
        let client = EntitlementClient::new(endpoint.clone(), verifier.clone(), Arc::new(MemoryTokenStore::default()));
        let config = CatalogConfig { features: [FEATURE_ENTITLEMENTS.to_owned()].into(), ..CatalogConfig::default() };
        let target = TokenTarget { base: "https://registry.example.test", config: Some(&config), package_id: PACKAGE, content_hash: HASH };
        let token = client.token_for(&target, &licensee, NOW).unwrap().held.token;
        let entitled = EntitlementGate::open(terms.clone(), PACKAGE, HASH, &licensee, Some(&token), verifier.as_ref(), NOW);
        assert_eq!((entitled.status(), entitled.status_label_at(NOW)), (GateStatus::Entitled { exp: NOW + 3600 }, "entitled"));
        assert_eq!(entitled.load_at("terrain_full", &sealed, NOW).unwrap().into_owned(), b"full-resolution tiles");
        assert_eq!(entitled.load_at("preview_1", &preview, NOW).unwrap(), Cow::Borrowed(&preview[..]));
        // A sealed entry presented under another id (or tampered) never falls back to its bytes.
        assert!(matches!(entitled.load_at("terrain_lite", &sealed, NOW), Err(GateRefusal::OpenFailed { .. })));
        // Past exp the same gate stops opening sealed entries; free entries still load.
        let after = NOW + 3600 + TOKEN_CLOCK_LEEWAY_SECS + 1;
        assert_eq!(entitled.status_label_at(after), "installed-preview");
        assert_eq!(
            entitled.load_at("terrain_full", &sealed, after),
            Err(GateRefusal::EntitlementRequired { entry_id: "terrain_full".into() })
        );

        // AC-4: the same token for another build, another device, or after exp is refused at open.
        let other_device = random_bytes();
        let stranger = Licensee { identity: &identity, device_secret: &other_device };
        let cases = [
            ("another build", OTHER_HASH, licensee, NOW, EntitlementError::ContentHashMismatch),
            ("another device", HASH, stranger, NOW, EntitlementError::WrongDevice),
            ("expired", HASH, licensee, after, EntitlementError::Expired { exp: NOW + 3600, now: after }),
        ];
        for (why, hash, holder, at, refusal) in cases {
            let gate = EntitlementGate::open(terms.clone(), PACKAGE, hash, &holder, Some(&token), verifier.as_ref(), at);
            assert_eq!(gate.status(), GateStatus::Preview(PreviewReason::Refused(refusal)), "{why}");
            assert!(gate.load_at("terrain_full", &sealed, at).is_err(), "{why}: never plaintext");
        }
    }
}
