//! Capability grant parsed from `manifest.json`'s `capabilities` array.
//!
//! Distinct from fe-plugin's scope-pattern `CapabilityManifest` (verse/petal/property
//! scopes) — this is the flat capability-string model the HOST-FN CONTRACT uses for
//! storage/query access. See AGENTS.md "Data contract & capabilities" for the mapping.

use std::collections::HashSet;

use serde::Deserialize;

/// Required to read node properties or extension key/value storage.
pub const STORAGE_READ: &str = "storage.read";
/// Required to write node properties or extension key/value storage.
pub const STORAGE_WRITE: &str = "storage.write";
/// Required to issue a `query_select` call.
pub const QUERY_SELECT: &str = "query.select";

/// The manifest's `capabilities` array, embedded at compile time.
const MANIFEST_SOURCE: &str = include_str!("../manifest.json");

#[derive(Deserialize)]
struct ManifestCapabilities {
    #[serde(default)]
    capabilities: Vec<String>,
}

/// A grant of capability strings (e.g. `"storage.read"`) held by this extension instance.
#[derive(Debug, Clone, Default)]
pub struct Capabilities(HashSet<String>);

impl Capabilities {
    /// Grant exactly the given capability strings.
    pub fn new(caps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(caps.into_iter().map(Into::into).collect())
    }

    /// No capabilities at all — used to test fail-closed behavior.
    pub fn empty() -> Self {
        Self(HashSet::new())
    }

    /// All three capabilities this extension requests (`storage.read/write`, `query.select`).
    pub fn full() -> Self {
        Self::new([STORAGE_READ, STORAGE_WRITE, QUERY_SELECT])
    }

    /// Parse the `capabilities` array out of a manifest.json document.
    pub fn from_manifest_json(json: &str) -> Result<Self, serde_json::Error> {
        let parsed: ManifestCapabilities = serde_json::from_str(json)?;
        Ok(Self::new(parsed.capabilities))
    }

    /// The grant declared by this extension's shipped `manifest.json`.
    pub fn from_shipped_manifest() -> Self {
        Self::from_manifest_json(MANIFEST_SOURCE).expect("manifest.json must be valid JSON")
    }

    /// Whether the given capability string is granted.
    pub fn has(&self, capability: &str) -> bool {
        self.0.contains(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_grants_all_three_capabilities() {
        let caps = Capabilities::full();
        assert!(caps.has(STORAGE_READ));
        assert!(caps.has(STORAGE_WRITE));
        assert!(caps.has(QUERY_SELECT));
    }

    #[test]
    fn empty_grants_nothing() {
        let caps = Capabilities::empty();
        assert!(!caps.has(STORAGE_READ));
    }

    #[test]
    fn shipped_manifest_matches_full_grant() {
        let caps = Capabilities::from_shipped_manifest();
        assert!(caps.has(STORAGE_READ));
        assert!(caps.has(STORAGE_WRITE));
        assert!(caps.has(QUERY_SELECT));
    }
}
