//! HTTP client for a hosted fe-hexon-registry — see `fe-hexon-registry/AGENTS.md` §client.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

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
        env.data.ok_or_else(|| anyhow!("registry response missing data"))
    }
}
