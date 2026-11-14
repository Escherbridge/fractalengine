//! Entitlement status and per-member gating for installed terrain tilesets (see `src/AGENTS.md` §tileset-entitlement).

use anyhow::Context;
use fe_format::entitlement::{unix_now, EntitlementVerifier};
use fe_format::integrity::MANIFEST_PATH;
use fe_format::{HexonArchiveIndex, License};
use fe_hexon::entitlement_client::Licensee;
use fe_hexon::entitlement_gate::{EntitlementGate, LicenseTerms};

use super::store::HexonStore;

/// Archive member holding the package licence.
pub const LICENSE_PATH: &str = "license.json";

/// A non-free tile member of a licensed tileset read without an entitlement (a state, not a fault).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tile member `{member}` needs an entitlement")]
pub struct EntitlementRequired {
    /// Archive member path, e.g. `terrain/tiles/12/2048/1361.png`.
    pub member: String,
}

/// True when `err` (anywhere in its chain) is an [`EntitlementRequired`] state rather than a read fault.
pub(crate) fn is_entitlement_refusal(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<EntitlementRequired>())
}

/// Hexon Manager view of one installed tileset's entitlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilesetEntitlement {
    /// `installed`, `installed-preview`, `entitlement-required` or `entitled`.
    pub label: &'static str,
    /// Publisher's payment page for a licensed tileset, shown as a plain hyperlink.
    pub payment_verification_url: Option<String>,
}

/// Typed form of [`TilesetEntitlement::label`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TilesetAccess {
    /// No licence, or a free/attribution one: every member loads.
    Installed,
    /// Licensed without a valid token: only the listed free members load.
    InstalledPreview,
    /// Licensed without a valid token and no free members: nothing loads.
    EntitlementRequired,
    /// Licensed with a verified, unexpired token: every member loads.
    Entitled,
}

impl TilesetAccess {
    /// Parse a gate status label; `None` for an unknown label.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "installed" => Some(Self::Installed),
            "installed-preview" => Some(Self::InstalledPreview),
            "entitlement-required" => Some(Self::EntitlementRequired),
            "entitled" => Some(Self::Entitled),
            _ => None,
        }
    }

    /// The gate status label this access reads as.
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::InstalledPreview => "installed-preview",
            Self::EntitlementRequired => "entitlement-required",
            Self::Entitled => "entitled",
        }
    }

    /// True when non-free members load.
    pub fn is_fully_loadable(self) -> bool {
        matches!(self, Self::Installed | Self::Entitled)
    }
}

/// The archive's licence through the capped, coverage-checked header reader; `None` without `license.json`.
pub fn read_license(index: &HexonArchiveIndex) -> anyhow::Result<Option<License>> {
    let Some(raw) = index.read_member(LICENSE_PATH)? else {
        return Ok(None);
    };
    serde_json::from_slice(&raw)
        .map(Some)
        .context("invalid license.json")
}

/// Licence terms of an optional licence (`None` = unlicensed).
pub fn license_terms(license: Option<&License>) -> LicenseTerms {
    license
        .map(LicenseTerms::from_format_license)
        .unwrap_or_else(LicenseTerms::unlicensed)
}

/// Open the entitlement gate for an installed tileset; `None` when it is not installed or its header does not read.
///
/// Unlicensed and free/attribution archives never consult `verifier` (NFR-2). Attach the returned
/// gate to the source from [`HexonStore::load_tileset`] with `HexonTileSource::set_entitlement_gate`.
pub fn tileset_entitlement(
    store: &HexonStore,
    hexon_id: &str,
    token: Option<&str>,
    licensee: &Licensee<'_>,
    verifier: &dyn EntitlementVerifier,
) -> Option<(TilesetEntitlement, EntitlementGate)> {
    let path = store.installed_archive_path(hexon_id)?;
    let index = match HexonArchiveIndex::open(&path) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(hexon_id, %error, "installed tileset header does not read; no entitlement status");
            return None;
        }
    };
    let license = match read_license(&index) {
        Ok(license) => license,
        Err(error) => {
            tracing::warn!(hexon_id, %error, "installed tileset licence does not read; no entitlement status");
            return None;
        }
    };
    let terms = license_terms(license.as_ref());
    let content_hash = if terms.is_licensed() {
        match manifest_content_hash(&index) {
            Ok(hash) => hash,
            Err(error) => {
                tracing::warn!(hexon_id, %error, "installed tileset content hash does not compute; no entitlement status");
                return None;
            }
        }
    } else {
        String::new()
    };
    let payment_verification_url = if terms.is_licensed() {
        license.and_then(|license| license.payment_verification_url)
    } else {
        None
    };
    let now = unix_now();
    let package_id = index.manifest().hexon_id.as_str();
    let gate = EntitlementGate::open(terms, package_id, &content_hash, licensee, token, verifier, now);
    let label = gate.status_label_at(now);
    Some((TilesetEntitlement { label, payment_verification_url }, gate))
}

/// The build's `content_hash`, recomputed from its own `manifest.json` (same rule as `archive_content_hash`).
fn manifest_content_hash(index: &HexonArchiveIndex) -> anyhow::Result<String> {
    let raw = index
        .read_member(MANIFEST_PATH)?
        .with_context(|| format!("missing {MANIFEST_PATH}"))?;
    fe_format::integrity::content_hash(&raw)
}
