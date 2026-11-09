//! Install-from-registry over registry protocol v1 (rationale: src/AGENTS.md §registry-install).

use fe_format::license::{License, LicenseType};
use fe_format::HexonArchive;

use crate::package::{payload_hash, SignatureEntry};

/// Registry protocol version this client speaks.
pub const SUPPORTED_PROTOCOL: u32 = 1;
/// Response header every `/api/v1/*` registry response carries.
pub const PROTOCOL_HEADER: &str = "X-Hexon-Registry-Protocol";
/// Plugin manifest member: the exact bytes the publisher signed.
pub const MANIFEST_MEMBER: &str = "plugin/manifest.json";
/// Plugin payload member: Wasm bytes, sealed under the content key for paid packages.
pub const PAYLOAD_MEMBER: &str = "plugin/payload.wasm";
/// Signature member: `{package_id, content_hash, signatures}`.
pub const SIGNATURES_MEMBER: &str = "plugin/signatures.json";

const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 1024 * 1024;

/// Typed registry install failures.
#[derive(Debug, thiserror::Error)]
pub enum RegistryInstallError {
    #[error("registry transport error: {0}")]
    Transport(String),
    #[error("registry speaks protocol {found:?}, this client requires 1")]
    ProtocolMismatch { found: Option<String> },
    #[error("package '{0}' not found on the registry")]
    NotFound(String),
    #[error("package is malformed: {0}")]
    Malformed(String),
    #[error("package '{0}' is licensed and no entitlement token was supplied")]
    EntitlementRequired(String),
    #[error("entitlement refused for '{package_id}': {reason}")]
    EntitlementRefused { package_id: String, reason: String },
    #[error("payload hash {actual} does not match the declared content hash {declared}")]
    ContentHashMismatch { declared: String, actual: String },
}

/// The `plugin/signatures.json` document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignatureDoc {
    pub package_id: String,
    /// Hex blake3 of the plaintext payload; also the entitlement token's `content_hash`.
    pub content_hash: String,
    pub signatures: Vec<SignatureEntry>,
}

/// A fetched, unwrapped plugin package, ready for `PluginManager::install_plugin`.
#[derive(Debug, Clone)]
pub struct FetchedPackage {
    pub package_id: String,
    pub manifest_bytes: Vec<u8>,
    pub payload: Vec<u8>,
    pub signatures: Vec<SignatureEntry>,
    pub content_hash: String,
}

/// Entitlement inputs for a `LicenseType::Paid` package.
pub struct EntitlementContext<'a> {
    pub verifier: &'a dyn fe_format::entitlement::EntitlementVerifier,
    pub token: &'a str,
}

/// Reads protocol v1 registries (`fe-hexon-registry` or any conforming store).
#[derive(Clone)]
pub struct RegistryClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RegistryClient {
    /// Client for `base_url` (trailing slash tolerated) with an optional bearer token.
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    fn get(&self, url: reqwest::Url) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => self.http.get(url).bearer_auth(t),
            None => self.http.get(url),
        }
    }

    fn url(&self, segments: &[&str]) -> Result<reqwest::Url, RegistryInstallError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|e| RegistryInstallError::Transport(format!("bad registry url: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| RegistryInstallError::Transport("registry url cannot be a base".into()))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    /// Refuses any registry that does not advertise protocol v1 on `/health`.
    pub async fn check_protocol(&self) -> Result<(), RegistryInstallError> {
        let response = self
            .get(self.url(&["health"])?)
            .send()
            .await
            .map_err(|e| RegistryInstallError::Transport(e.to_string()))?;
        let header = header_value(&response);
        if protocol_matches(header.as_deref()) {
            return Ok(());
        }
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let from_body = body
            .pointer("/data/protocol_version")
            .or_else(|| body.get("protocol_version"))
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string());
        if protocol_matches(from_body.as_deref()) {
            Ok(())
        } else {
            Err(RegistryInstallError::ProtocolMismatch { found: header.or(from_body) })
        }
    }

    /// Downloads the sealed package archive, re-checking the protocol header on the download response.
    pub async fn download(&self, uri: &str) -> Result<Vec<u8>, RegistryInstallError> {
        self.check_protocol().await?;
        let mut response = self
            .get(self.url(&["api", "v1", "hexons", uri, "download"])?)
            .send()
            .await
            .map_err(|e| RegistryInstallError::Transport(e.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryInstallError::NotFound(uri.to_string()));
        }
        if !response.status().is_success() {
            return Err(RegistryInstallError::Transport(format!("download status {}", response.status())));
        }
        let header = header_value(&response);
        if !protocol_matches(header.as_deref()) {
            return Err(RegistryInstallError::ProtocolMismatch { found: header });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| RegistryInstallError::Transport(e.to_string()))?
        {
            if body.len() as u64 + chunk.len() as u64 > MAX_PACKAGE_BYTES {
                return Err(RegistryInstallError::Malformed("package exceeds the size cap".into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Download and unwrap a plugin package; signature, revocation and activation stay with `PluginManager`.
    pub async fn fetch_package(
        &self,
        uri: &str,
        entitlement: Option<EntitlementContext<'_>>,
    ) -> Result<FetchedPackage, RegistryInstallError> {
        let bytes = self.download(uri).await?;
        unwrap_package(&bytes, entitlement)
    }
}

fn header_value(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(PROTOCOL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn protocol_matches(value: Option<&str>) -> bool {
    value.and_then(|v| v.trim().parse::<u32>().ok()) == Some(SUPPORTED_PROTOCOL)
}

fn read_required(bytes: &[u8], name: &str, cap: u64) -> Result<Vec<u8>, RegistryInstallError> {
    HexonArchive::read_member(bytes, name, cap)
        .map_err(|e| RegistryInstallError::Malformed(format!("{name}: {e}")))?
        .ok_or_else(|| RegistryInstallError::Malformed(format!("missing member {name}")))
}

/// Extracts manifest, payload and signatures; paid packages open only through the existing `EntitlementVerifier`.
pub fn unwrap_package(
    archive_bytes: &[u8],
    entitlement: Option<EntitlementContext<'_>>,
) -> Result<FetchedPackage, RegistryInstallError> {
    let manifest_bytes = read_required(archive_bytes, MANIFEST_MEMBER, MAX_JSON_BYTES)?;
    let doc: SignatureDoc =
        serde_json::from_slice(&read_required(archive_bytes, SIGNATURES_MEMBER, MAX_JSON_BYTES)?)
            .map_err(|e| RegistryInstallError::Malformed(format!("{SIGNATURES_MEMBER}: {e}")))?;
    let stored_payload = read_required(archive_bytes, PAYLOAD_MEMBER, MAX_PACKAGE_BYTES)?;

    let license: Option<License> = HexonArchive::read_member(archive_bytes, "license.json", MAX_JSON_BYTES)
        .map_err(|e| RegistryInstallError::Malformed(format!("license.json: {e}")))?
        .map(|raw| serde_json::from_slice(&raw))
        .transpose()
        .map_err(|e| RegistryInstallError::Malformed(format!("license.json: {e}")))?;

    let payload = match license.as_ref().map(|l| &l.license_type) {
        Some(LicenseType::Paid) => {
            let context = entitlement
                .ok_or_else(|| RegistryInstallError::EntitlementRequired(doc.package_id.clone()))?;
            unlock_paid_payload(&doc, &stored_payload, context)?
        }
        _ => stored_payload,
    };

    let actual = payload_hash(&payload);
    if actual != doc.content_hash {
        return Err(RegistryInstallError::ContentHashMismatch { declared: doc.content_hash, actual });
    }
    Ok(FetchedPackage {
        package_id: doc.package_id,
        manifest_bytes,
        payload,
        signatures: doc.signatures,
        content_hash: doc.content_hash,
    })
}

/// The single call site onto the reused verifier: token -> content key -> sealed payload.
fn unlock_paid_payload(
    doc: &SignatureDoc,
    sealed_payload: &[u8],
    context: EntitlementContext<'_>,
) -> Result<Vec<u8>, RegistryInstallError> {
    let refused = |reason: String| RegistryInstallError::EntitlementRefused {
        package_id: doc.package_id.clone(),
        reason,
    };
    let content_key = context
        .verifier
        .verify(context.token, &doc.package_id, &doc.content_hash)
        .map_err(|e| refused(e.to_string()))?;
    fe_format::entitlement::open_paid_entry(&content_key, sealed_payload).map_err(|e| refused(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_header_table() {
        for (value, ok) in [(Some("1"), true), (Some(" 1 "), true), (Some("2"), false), (Some("x"), false), (None, false)] {
            assert_eq!(protocol_matches(value), ok, "{value:?}");
        }
    }
}
