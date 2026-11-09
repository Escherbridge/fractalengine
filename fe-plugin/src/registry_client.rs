//! Install-from-registry over registry protocol v1 (rationale: src/AGENTS.md §registry-install).

use std::time::Duration;

use fe_format::license::{License, LicenseType};
use fe_format::HexonArchive;

use crate::package::{payload_hash, SignatureEntry};

/// Registry protocol version this client speaks.
pub const SUPPORTED_PROTOCOL: u32 = 1;
/// Protocol header a registry may send; never required (store-client contract v1 reads `config.json`).
pub const PROTOCOL_HEADER: &str = "X-Hexon-Registry-Protocol";
/// Plugin manifest member: the exact bytes the publisher signed.
pub const MANIFEST_MEMBER: &str = "plugin/manifest.json";
/// Plugin payload member: the Wasm core module.
pub const PAYLOAD_MEMBER: &str = "plugin/payload.wasm";
/// Signature member: `{package_id, content_hash, signatures}`.
pub const SIGNATURES_MEMBER: &str = "plugin/signatures.json";

const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_HEALTH_BYTES: u64 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

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
    #[error("requested package '{requested}' but the registry served '{served}'")]
    IdentityMismatch { requested: String, served: String },
    #[error("package '{0}' is licensed and this host cannot verify entitlements yet")]
    EntitlementUnavailable(String),
    #[error("payload hash {actual} does not match the declared content hash {declared}")]
    ContentHashMismatch { declared: String, actual: String },
    #[error("manifest hash {actual:?} does not match the registry's listed content hash {listed}")]
    ListedHashMismatch { listed: String, actual: Option<String> },
}

/// The `plugin/signatures.json` document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignatureDoc {
    pub package_id: String,
    /// Hex blake3 of the plaintext payload.
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
    /// Store-client contract v1 `content_hash`: BLAKE3 of the archive's `manifest.json` with the signature blanked; `None` when the archive has no manifest.
    pub manifest_hash: Option<String>,
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
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "registry client: timeouts unavailable, using the default client");
                reqwest::Client::new()
            });
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http,
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

    /// Refuses any registry that advertises no protocol v1: `/health` header or body, else `config.json` `registry_protocol`.
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
        let body: serde_json::Value = read_capped(response, MAX_HEALTH_BYTES)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let from_body = body
            .pointer("/data/protocol_version")
            .or_else(|| body.get("protocol_version"))
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string());
        if protocol_matches(from_body.as_deref()) {
            return Ok(());
        }
        let from_config = self.config_protocol().await;
        if protocol_matches(from_config.as_deref()) {
            Ok(())
        } else {
            Err(RegistryInstallError::ProtocolMismatch { found: header.or(from_body).or(from_config) })
        }
    }

    /// `registry_protocol` from the public `config.json`; `None` when absent or unreadable.
    async fn config_protocol(&self) -> Option<String> {
        let response = self.get(self.url(&["config.json"]).ok()?).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = read_capped(response, MAX_HEALTH_BYTES).await.ok()?;
        let config: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        config.get("registry_protocol").and_then(|v| v.as_u64()).map(|v| v.to_string())
    }

    /// Downloads the sealed package archive; a protocol header on the response, when sent, must say 1.
    pub async fn download(&self, uri: &str) -> Result<Vec<u8>, RegistryInstallError> {
        self.check_protocol().await?;
        let response = self
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
        if let Some(found) = header_value(&response) {
            if !protocol_matches(Some(found.as_str())) {
                return Err(RegistryInstallError::ProtocolMismatch { found: Some(found) });
            }
        }
        read_capped(response, MAX_PACKAGE_BYTES).await
    }

    /// Download and unwrap a plugin package served for `uri` (`{id}` or `{id}@{version}`); trust stays with `PluginManager`.
    pub async fn fetch_package(&self, uri: &str) -> Result<FetchedPackage, RegistryInstallError> {
        let bytes = self.download(uri).await?;
        let package = unwrap_package(&bytes)?;
        let requested = uri.split_once('@').map_or(uri, |(id, _)| id);
        if package.package_id != requested {
            return Err(RegistryInstallError::IdentityMismatch {
                requested: requested.to_string(),
                served: package.package_id,
            });
        }
        Ok(package)
    }

    /// [`Self::fetch_package`] that also requires the archive's manifest hash to equal the `content_hash` the registry listed (fail closed).
    pub async fn fetch_package_listed(&self, uri: &str, listed_content_hash: &str) -> Result<FetchedPackage, RegistryInstallError> {
        let package = self.fetch_package(uri).await?;
        match package.manifest_hash.as_deref() {
            Some(actual) if actual.eq_ignore_ascii_case(listed_content_hash) => Ok(package),
            other => Err(RegistryInstallError::ListedHashMismatch {
                listed: listed_content_hash.to_string(),
                actual: other.map(str::to_string),
            }),
        }
    }
}

/// Reads a response body, refusing it past `cap` bytes.
async fn read_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>, RegistryInstallError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| RegistryInstallError::Transport(e.to_string()))?
    {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(RegistryInstallError::Malformed("response exceeds the size cap".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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

/// Extracts manifest, payload and signatures; a `paid` licence is refused until an entitlement verifier exists.
pub fn unwrap_package(archive_bytes: &[u8]) -> Result<FetchedPackage, RegistryInstallError> {
    let manifest_bytes = read_required(archive_bytes, MANIFEST_MEMBER, MAX_JSON_BYTES)?;
    let doc: SignatureDoc =
        serde_json::from_slice(&read_required(archive_bytes, SIGNATURES_MEMBER, MAX_JSON_BYTES)?)
            .map_err(|e| RegistryInstallError::Malformed(format!("{SIGNATURES_MEMBER}: {e}")))?;

    let license: Option<License> = HexonArchive::read_member(archive_bytes, "license.json", MAX_JSON_BYTES)
        .map_err(|e| RegistryInstallError::Malformed(format!("license.json: {e}")))?
        .map(|raw| serde_json::from_slice(&raw))
        .transpose()
        .map_err(|e| RegistryInstallError::Malformed(format!("license.json: {e}")))?;
    if matches!(license.as_ref().map(|l| &l.license_type), Some(LicenseType::Paid)) {
        return Err(RegistryInstallError::EntitlementUnavailable(doc.package_id));
    }

    let manifest_hash = HexonArchive::read_member(archive_bytes, fe_format::integrity::MANIFEST_PATH, MAX_JSON_BYTES)
        .map_err(|e| RegistryInstallError::Malformed(format!("manifest.json: {e}")))?
        .map(|raw| fe_format::integrity::content_hash(&raw))
        .transpose()
        .map_err(|e| RegistryInstallError::Malformed(format!("manifest.json: {e:#}")))?;

    let payload = read_required(archive_bytes, PAYLOAD_MEMBER, MAX_PACKAGE_BYTES)?;
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
        manifest_hash,
    })
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
