//! HTTP client for a hosted fe-hexon-registry — see `fe-hexon-registry/AGENTS.md` §client.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use fe_format::{limits, HeaderDoc};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::partial::{FetchSource, PartialFetchError, PartialHexonFetch};
use crate::registry_source::{
    valid_package_id, valid_version, ProtocolV1Source, ProtocolV1Transport, RegistryEntry, SourceTrust, PACKAGE_MAX_BYTES,
};

/// Response cap for `/meta`: every header member at its JSON cap, plus the manifest.
const MAX_HEADER_RESPONSE_BYTES: u64 = 8 * limits::MAX_JSON_ENTRY_BYTES;
/// Response cap for `/member`: one member at the binary entry cap.
const MAX_MEMBER_RESPONSE_BYTES: u64 = limits::MAX_ENTRY_UNCOMPRESSED_BYTES;
/// Response cap for `/chunk` (a monolithic `seq = 0` is the whole archive).
const MAX_CHUNK_RESPONSE_BYTES: u64 = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
/// A registry that does not accept the connection in this time is unreachable.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Longest silence between body reads; an idle timeout, so a large tileset still streams.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// `hexon_type` of an extension package (`HexonType::Plugin`).
const PLUGIN_HEXON_TYPE: &str = "plugin";
/// Redirect hops a store or registry read follows (same origin only).
const MAX_REDIRECTS: usize = 5;

/// Redirects stay on the origin of the first request: never another host, never https to http, so a redirect
/// cannot carry a read to loopback or the LAN unless the configured base already is there.
pub fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let hops = attempt.previous().len();
        let same_origin = attempt.previous().first().is_some_and(|first| first.origin() == attempt.url().origin());
        if hops >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if same_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// The one client builder every store, registry and revocation reader starts from: connect and read-idle timeouts,
/// no environment proxy (the address checks must see the real target) and same-origin redirects. Callers add their
/// own checked resolver (`dns_resolver`) on top; nothing here sets credentials or a cookie store.
pub fn store_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .no_proxy()
        .redirect(same_origin_redirects())
}

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
        // A stalled registry must not hold the extension ops worker forever; a proxy or a cross-origin redirect
        // must not re-target it. The fallback (TLS backend failure) still has no proxy and follows no redirect.
        let http = store_client_builder().build().unwrap_or_else(|error| {
            tracing::warn!(%error, "registry client: hardened client unavailable, requests are refused");
            reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default()
        });
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    /// GET with bearer auth applied when configured.
    fn get(&self, url: String) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => self.http.get(url).bearer_auth(t),
            None => self.http.get(url),
        }
    }

    /// POST with bearer auth applied when configured.
    fn post(&self, url: String) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => self.http.post(url).bearer_auth(t),
            None => self.http.post(url),
        }
    }

    /// Publish sealed `.hexon` bytes (raw body, as the registry's publish route reads it); returns the registry's receipt.
    pub async fn publish(&self, bytes: Vec<u8>) -> Result<serde_json::Value> {
        let resp = self
            .post(format!("{}/api/v1/hexons/publish", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .context("registry publish request failed")?;
        Self::unwrap_envelope(resp).await
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

    /// Download the full `.hexon` archive bytes for `uri`, never past the whole-archive cap.
    pub async fn download(&self, uri: &str) -> Result<Vec<u8>> {
        self.download_capped(uri, limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES).await
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
        let advertised = doc
            .pointer("/data/protocol_version")
            .or_else(|| doc.get("protocol_version"))
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string());
        if advertised.is_some() {
            return Ok(advertised);
        }
        // Store-client contract v1: `{base}/api/v1/catalog/config.json` `registry_protocol` is the authority; the
        // header is never required. Catalogue reads are anonymous: no bearer token, whatever the operator configured.
        let config = self
            .http
            .get(format!("{}/{}/config.json", self.base_url, crate::registry_source::CATALOG_PREFIX))
            .send()
            .await
            .context("registry config request failed")?;
        if !config.status().is_success() {
            return Ok(None);
        }
        let body = self.get_capped_response(config, MAX_HEALTH_BYTES).await?;
        match crate::registry_source::parse_config(&body) {
            Ok(_) => Ok(Some(crate::registry_source::PROTOCOL_V1.to_string())),
            Err(crate::registry_source::RegistryError::ProtocolMismatch { found }) => Ok(found),
            Err(error) => Err(anyhow!("registry config: {error}")),
        }
    }

    /// Attestation document for `uri`: the manifest and header members from `/meta`, base64-encoded, so consent renders before download.
    pub async fn attestation(&self, entry: &RegistryEntry) -> Result<Vec<u8>, PartialFetchError> {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let doc = self.fetch_header(&entry.url).await?;
        let content_hash = fe_format::integrity::content_hash(doc.manifest.as_bytes())
            .map_err(|e| PartialFetchError::Transport(format!("manifest: {e:#}")))?;
        let members: std::collections::BTreeMap<&String, String> =
            doc.header_members.iter().map(|(path, text)| (path, engine.encode(text.as_bytes()))).collect();
        let attestation = serde_json::json!({
            "package_id": entry.package_id,
            "version": entry.version,
            "content_hash": content_hash,
            "manifest_b64": engine.encode(doc.manifest.as_bytes()),
            "header_members_b64": members,
        });
        serde_json::to_vec(&attestation).map_err(|e| PartialFetchError::Transport(e.to_string()))
    }

    /// Read a response body under `cap` bytes.
    async fn get_capped_response(&self, resp: reqwest::Response, cap: u64) -> Result<Vec<u8>> {
        self.get_capped_response_with_progress(resp, cap, 0, &mut |_, _| {}).await
    }

    /// [`Self::get_capped_response`] reporting `(received, total)`, throttled, with a final `(received, received)`.
    /// `total` is Content-Length, else `expected_total` (0 = unknown); a Content-Length past `cap` is refused unread.
    /// Generic over the callback so no-op callers keep a `Send` future.
    async fn get_capped_response_with_progress<F: FnMut(u64, u64) + ?Sized>(
        &self,
        mut resp: reqwest::Response,
        cap: u64,
        expected_total: u64,
        on_bytes: &mut F,
    ) -> Result<Vec<u8>> {
        let declared = resp.content_length();
        if declared.is_some_and(|length| length > cap) {
            return Err(anyhow!("registry response exceeds {cap} bytes"));
        }
        let total = declared.unwrap_or(expected_total.min(cap));
        let step = (total / 100).max(64 * 1024);
        on_bytes(0, total);
        let mut body = Vec::new();
        let mut last_reported = 0u64;
        while let Some(piece) = resp.chunk().await? {
            if (body.len() + piece.len()) as u64 > cap {
                return Err(anyhow!("registry response exceeds {cap} bytes"));
            }
            body.extend_from_slice(&piece);
            let received = body.len() as u64;
            if received - last_reported >= step && (total == 0 || received < total) {
                last_reported = received;
                on_bytes(received, total);
            }
        }
        let received = body.len() as u64;
        on_bytes(received, received);
        Ok(body)
    }

    /// [`Self::download`] that stops reading past `cap` bytes, so a registry cannot stream an unbounded body.
    pub async fn download_capped(&self, uri: &str, cap: u64) -> Result<Vec<u8>> {
        self.download_capped_with_progress(uri, cap, 0, &mut |_, _| {}).await
    }

    /// [`Self::download_capped`] reporting byte progress to `on_bytes`; `expected_total` is the listed size (0 = unknown).
    pub async fn download_capped_with_progress<F: FnMut(u64, u64) + ?Sized>(
        &self,
        uri: &str,
        cap: u64,
        expected_total: u64,
        on_bytes: &mut F,
    ) -> Result<Vec<u8>> {
        let resp = self
            .get(format!("{}/api/v1/hexons/{uri}/download", self.base_url))
            .send()
            .await
            .context("registry download request failed")?;
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("registry download failed: {status}"));
        }
        self.get_capped_response_with_progress(resp, cap, expected_total, on_bytes).await
    }
}

/// A registry uri is `id[@version]` in the contract v1 grammar; anything that could re-target the request is refused.
fn is_plain_registry_uri(uri: &str) -> bool {
    match uri.split_once('@') {
        Some((id, version)) => valid_package_id(id) && valid_version(version),
        None => valid_package_id(uri),
    }
}

/// Bytes a download may read: the listed `size` when the registry gave one, never past the package cap.
fn download_cap(entry: &RegistryEntry) -> u64 {
    match entry.size {
        0 => PACKAGE_MAX_BYTES,
        listed => listed.min(PACKAGE_MAX_BYTES),
    }
}

/// Synchronous [`ProtocolV1Transport`] over [`RemoteRegistryClient`]; call from a worker thread, never from async code.
pub struct RemoteProtocolV1Transport {
    client: RemoteRegistryClient,
    runtime: tokio::runtime::Runtime,
    /// When set, listings keep only hits of this `hexon_type` (asked of the registry and re-checked here).
    hexon_type: Option<&'static str>,
}

impl RemoteProtocolV1Transport {
    /// Transport for `base_url`; downloads are anonymous unless the operator configured a registry token.
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        Ok(Self { client: RemoteRegistryClient::new(base_url, token), runtime, hexon_type: None })
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
            .block_on(self.client.search(query, None, self.hexon_type))
            .map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .filter(|hit| valid_package_id(&hit.hexon_id) && valid_version(&hit.version))
            .filter(|hit| self.hexon_type.is_none_or(|wanted| hit.hexon_type == wanted))
            .map(entry_from_summary)
            .collect())
    }

    fn download(&self, entry: &RegistryEntry) -> Result<Vec<u8>, String> {
        self.download_with_progress(entry, &mut |_, _| {})
    }

    fn download_with_progress(&self, entry: &RegistryEntry, on_bytes: &mut dyn FnMut(u64, u64)) -> Result<Vec<u8>, String> {
        if !is_plain_registry_uri(&entry.url) {
            return Err(format!("refusing registry uri {:?}", entry.url));
        }
        let cap = download_cap(entry);
        // A listed size is the progress total when the response omits Content-Length; unlisted stays unknown (0).
        let expected_total = if entry.size == 0 { 0 } else { cap };
        self.runtime
            .block_on(self.client.download_capped_with_progress(&entry.url, cap, expected_total, on_bytes))
            .map_err(|e| e.to_string())
    }

    fn attestation(&self, entry: &RegistryEntry) -> Result<Option<Vec<u8>>, String> {
        if !is_plain_registry_uri(&entry.url) {
            return Err(format!("refusing registry uri {:?}", entry.url));
        }
        match self.runtime.block_on(self.client.attestation(entry)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(PartialFetchError::NotFound) => Ok(None),
            Err(other) => Err(other.to_string()),
        }
    }
}

/// A protocol-v1 registry as a [`RegistrySource`](crate::registry_source::RegistrySource): every download re-verified
/// locally; `fetch_hexon` also admits content hexons such as tilesets (see `src/AGENTS.md` §registry-source-remote).
pub fn remote_source(
    label: impl Into<String>,
    base_url: impl Into<String>,
    token: Option<String>,
    trust: SourceTrust,
) -> std::io::Result<ProtocolV1Source> {
    let transport = RemoteProtocolV1Transport::new(base_url, token)?;
    Ok(ProtocolV1Source::new(label, std::sync::Arc::new(transport), trust))
}

/// The Extensions Browse source over a protocol-v1 registry: anonymous by construction, listing only plugin packages.
pub fn remote_extension_source(
    label: impl Into<String>,
    base_url: impl Into<String>,
    trust: SourceTrust,
) -> std::io::Result<ProtocolV1Source> {
    let mut transport = RemoteProtocolV1Transport::new(base_url, None)?;
    transport.hexon_type = Some(PLUGIN_HEXON_TYPE);
    Ok(ProtocolV1Source::new(label, std::sync::Arc::new(transport), trust))
}
