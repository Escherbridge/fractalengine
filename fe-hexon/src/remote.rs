//! HTTP client for a hosted fe-hexon-registry — see `fe-hexon-registry/AGENTS.md` §client.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use fe_format::{limits, HeaderDoc};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::partial::{FetchSource, PartialFetchError, PartialHexonFetch};
use crate::registry_source::{ProtocolV1Source, ProtocolV1Transport, RegistryEntry, SourceTrust};

/// Response cap for `/meta`: every header member at its JSON cap, plus the manifest.
const MAX_HEADER_RESPONSE_BYTES: u64 = 8 * limits::MAX_JSON_ENTRY_BYTES;
/// Response cap for `/member`: one member at the binary entry cap.
const MAX_MEMBER_RESPONSE_BYTES: u64 = limits::MAX_ENTRY_UNCOMPRESSED_BYTES;
/// Response cap for `/chunk` (a monolithic `seq = 0` is the whole archive).
const MAX_CHUNK_RESPONSE_BYTES: u64 = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;

/// Mirror of the registry's `ApiResponse` envelope.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    #[serde(default = "Option::default")]
    data: Option<T>,
    #[serde(default)]
    error: Option<String>,
}

/// One search hit from the registry index (unknown fields ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteHexonSummary {
    pub hexon_id: String,
    pub version: String,
    pub hexon_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub publisher_did: String,
    #[serde(default)]
    pub size_bytes: u64,
    /// BLAKE3 of the sealed manifest with the signature blanked; empty when the registry omits it.
    #[serde(default)]
    pub content_hash: String,
}

/// Client for the hosted hexon registry HTTP API.
pub struct RemoteRegistryClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RemoteRegistryClient {
    /// Create a client for `base_url` (trailing slash tolerated) with optional bearer token.
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    /// GET with bearer auth applied when configured.
    fn get(&self, url: String) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => self.http.get(url).bearer_auth(t),
            None => self.http.get(url),
        }
    }

    /// Search the registry (`q` substring, `tags` comma-list, `hexon_type` exact); all optional.
    pub async fn search(
        &self,
        q: Option<&str>,
        tags: Option<&str>,
        hexon_type: Option<&str>,
    ) -> Result<Vec<RemoteHexonSummary>> {
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(q) = q {
            params.push(("q", q));
        }
        if let Some(tags) = tags {
            params.push(("tags", tags));
        }
        if let Some(t) = hexon_type {
            params.push(("type", t));
        }
        let resp = self
            .get(format!("{}/api/v1/hexons/search", self.base_url))
            .query(&params)
            .send()
            .await
            .context("registry search request failed")?;
        Self::unwrap_envelope(resp).await
    }

    /// Fetch the raw manifest for `uri` (`hexon_id` or `hexon_id@version`).
    pub async fn manifest(&self, uri: &str) -> Result<serde_json::Value> {
        let resp = self
            .get(format!("{}/api/v1/hexons/{uri}", self.base_url))
            .send()
            .await
            .context("registry manifest request failed")?;
        Self::unwrap_envelope(resp).await
    }

    /// Download the full `.hexon` archive bytes for `uri` (fetch-and-install path).
    pub async fn download(&self, uri: &str) -> Result<Vec<u8>> {
        let resp = self
            .get(format!("{}/api/v1/hexons/{uri}/download", self.base_url))
            .send()
            .await
            .context("registry download request failed")?;
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("registry download failed: {status}"));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// `{base}/api/v1/hexons/{uri}/{tail…}` with every segment percent-encoded.
    fn hexon_url(&self, uri: &str, tail: &[&str]) -> Result<reqwest::Url, PartialFetchError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|e| PartialFetchError::Transport(format!("bad registry url: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| PartialFetchError::Transport("registry url cannot be a base".into()))?
            .pop_if_empty()
            .extend(["api", "v1", "hexons", uri])
            .extend(tail);
        Ok(url)
    }

    /// GET `url`, mapping 404 to `NotFound`, and read at most `cap` body bytes.
    async fn get_capped(&self, url: reqwest::Url, cap: u64) -> Result<Bytes, PartialFetchError> {
        let transport = |e: reqwest::Error| PartialFetchError::Transport(e.to_string());
        let mut resp = self.get(url.to_string()).send().await.map_err(transport)?;
        match resp.status() {
            StatusCode::NOT_FOUND => return Err(PartialFetchError::NotFound),
            status if !status.is_success() => {
                return Err(PartialFetchError::Transport(format!(
                    "registry answered {status}"
                )))
            }
            _ => {}
        }
        if resp.content_length().is_some_and(|len| len > cap) {
            return Err(PartialFetchError::Transport(format!(
                "registry response exceeds {cap} bytes"
            )));
        }
        let mut body = Vec::new();
        while let Some(piece) = resp.chunk().await.map_err(transport)? {
            if (body.len() + piece.len()) as u64 > cap {
                return Err(PartialFetchError::Transport(format!(
                    "registry response exceeds {cap} bytes"
                )));
            }
            body.extend_from_slice(&piece);
        }
        Ok(Bytes::from(body))
    }

    /// Decode an `ApiResponse` envelope, surfacing registry-side errors.
    async fn unwrap_envelope<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        let env: Envelope<T> = resp
            .json()
            .await
            .with_context(|| format!("invalid registry response (HTTP {status})"))?;
        if !status.is_success() || !env.ok {
            return Err(anyhow!(
                "registry error (HTTP {status}): {}",
                env.error.unwrap_or_else(|| "unknown".into())
            ));
        }
        env.data
            .ok_or_else(|| anyhow!("registry response missing data"))
    }
}

/// HTTP transport over the registry's partial routes (`/meta`, `/chunk/{seq}`,
/// `/member/{*path}`); bytes are returned unverified — `FetchStrategy` checks them.
#[async_trait]
impl PartialHexonFetch for RemoteRegistryClient {
    fn source(&self) -> FetchSource {
        FetchSource::Registry {
            base_url: self.base_url.clone(),
        }
    }

    async fn fetch_header(&self, uri: &str) -> Result<HeaderDoc, PartialFetchError> {
        let url = self.hexon_url(uri, &["meta"])?;
        let body = self.get_capped(url, MAX_HEADER_RESPONSE_BYTES).await?;
        let envelope: Envelope<HeaderDoc> = serde_json::from_slice(&body)
            .map_err(|e| PartialFetchError::Transport(format!("invalid /meta response: {e}")))?;
        match envelope {
            Envelope {
                ok: true,
                data: Some(doc),
                ..
            } => Ok(doc),
            Envelope { error, .. } => Err(PartialFetchError::Transport(format!(
                "registry /meta error: {}",
                error.unwrap_or_else(|| "missing data".into())
            ))),
        }
    }

    async fn fetch_chunk(&self, uri: &str, seq: u32) -> Result<Bytes, PartialFetchError> {
        let seq = seq.to_string();
        let url = self.hexon_url(uri, &["chunk", seq.as_str()])?;
        self.get_capped(url, MAX_CHUNK_RESPONSE_BYTES).await
    }

    async fn fetch_member(&self, uri: &str, path: &str) -> Result<Bytes, PartialFetchError> {
        let components: Vec<&str> = path.split('/').collect();
        // No legitimate member path has an empty or dot component; refusing them
        // here keeps URL normalization from ever rewriting the request.
        if components
            .iter()
            .any(|c| c.is_empty() || *c == "." || *c == "..")
        {
            return Err(PartialFetchError::NotFound);
        }
        let mut tail = vec!["member"];
        tail.extend(components);
        let url = self.hexon_url(uri, &tail)?;
        self.get_capped(url, MAX_MEMBER_RESPONSE_BYTES).await
    }
}

/// Largest `/health` body read while probing the protocol.
const MAX_HEALTH_BYTES: u64 = 16 * 1024;

impl RemoteRegistryClient {
    /// Protocol version the registry advertises: `X-Hexon-Registry-Protocol`, else the `/health` body.
    pub async fn protocol(&self) -> Result<Option<String>> {
        let resp = self
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .context("registry health request failed")?;
        if let Some(header) = resp
            .headers()
            .get("x-hexon-registry-protocol")
            .and_then(|v| v.to_str().ok())
        {
            return Ok(Some(header.trim().to_string()));
        }
        let body = self.get_capped_response(resp, MAX_HEALTH_BYTES).await?;
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        Ok(doc
            .pointer("/data/protocol_version")
            .or_else(|| doc.get("protocol_version"))
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string()))
    }

    /// Read a response body under `cap` bytes.
    async fn get_capped_response(&self, mut resp: reqwest::Response, cap: u64) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        while let Some(piece) = resp.chunk().await? {
            if (body.len() + piece.len()) as u64 > cap {
                return Err(anyhow!("registry response exceeds {cap} bytes"));
            }
            body.extend_from_slice(&piece);
        }
        Ok(body)
    }
}

/// A registry uri is `hexon_id[@version]`; anything that could re-target the request is refused.
fn is_plain_registry_uri(uri: &str) -> bool {
    !uri.is_empty()
        && uri.len() <= 256
        && !uri.contains("..")
        && uri
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'))
}

/// Synchronous [`ProtocolV1Transport`] over [`RemoteRegistryClient`]; call from a worker thread, never from async code.
pub struct RemoteProtocolV1Transport {
    client: RemoteRegistryClient,
    runtime: tokio::runtime::Runtime,
}

impl RemoteProtocolV1Transport {
    /// Transport for `base_url`; downloads are anonymous unless the operator configured a registry token.
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        Ok(Self { client: RemoteRegistryClient::new(base_url, token), runtime })
    }
}

fn entry_from_summary(summary: RemoteHexonSummary) -> RegistryEntry {
    RegistryEntry {
        package_id: summary.hexon_id.clone(),
        version: summary.version.clone(),
        content_hash: summary.content_hash,
        url: format!("{}@{}", summary.hexon_id, summary.version),
        size: summary.size_bytes,
        name: summary.name,
        header_url: None,
    }
}

impl ProtocolV1Transport for RemoteProtocolV1Transport {
    fn protocol(&self) -> Result<Option<String>, String> {
        self.runtime.block_on(self.client.protocol()).map_err(|e| e.to_string())
    }

    fn search(&self, query: Option<&str>) -> Result<Vec<RegistryEntry>, String> {
        let hits = self
            .runtime
            .block_on(self.client.search(query, None, None))
            .map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .filter(|hit| is_plain_registry_uri(&hit.hexon_id) && is_plain_registry_uri(&hit.version))
            .map(entry_from_summary)
            .collect())
    }

    fn download(&self, entry: &RegistryEntry) -> Result<Vec<u8>, String> {
        if !is_plain_registry_uri(&entry.url) {
            return Err(format!("refusing registry uri {:?}", entry.url));
        }
        self.runtime.block_on(self.client.download(&entry.url)).map_err(|e| e.to_string())
    }
}

/// The Hexon Manager's remote source: protocol v1 over `base_url`, every download re-verified locally.
pub fn remote_source(
    label: impl Into<String>,
    base_url: impl Into<String>,
    token: Option<String>,
    trust: SourceTrust,
) -> std::io::Result<ProtocolV1Source> {
    let transport = RemoteProtocolV1Transport::new(base_url, token)?;
    Ok(ProtocolV1Source::new(label, std::sync::Arc::new(transport), trust))
}
