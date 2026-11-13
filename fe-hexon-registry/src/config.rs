//! Env-driven service configuration — see `AGENTS.md` §env config.

use std::path::PathBuf;

/// Runtime configuration resolved from `FE_REGISTRY_*` environment variables.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// Directory scanned for `*.hexon` packages (`FE_REGISTRY_DIR`, default `/data/hexons`).
    pub dir: PathBuf,
    /// Listen address (`FE_REGISTRY_BIND`, default `0.0.0.0:8790`).
    pub bind: String,
    /// Optional bearer token gating all `/api/v1` routes (`FE_REGISTRY_TOKEN`).
    pub token: Option<String>,
    /// When true, `publish` is rejected (`FE_REGISTRY_READONLY`, default false).
    pub readonly: bool,
    /// Explicit opt-in to run with no token (`FE_REGISTRY_ALLOW_OPEN=1`); without it a tokenless registry refuses to start.
    pub allow_open: bool,
    /// Serve every stored archive on `/blobs`, catalogue-listed or not (`FE_REGISTRY_PUBLIC_BLOBS=1`, default off).
    pub public_blobs: bool,
}

impl RegistryConfig {
    /// Read configuration from the environment, applying defaults.
    pub fn from_env() -> Self {
        Self {
            dir: std::env::var("FE_REGISTRY_DIR")
                .unwrap_or_else(|_| "/data/hexons".to_string())
                .into(),
            bind: std::env::var("FE_REGISTRY_BIND").unwrap_or_else(|_| "0.0.0.0:8790".to_string()),
            token: std::env::var("FE_REGISTRY_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
            readonly: std::env::var("FE_REGISTRY_READONLY")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            allow_open: std::env::var("FE_REGISTRY_ALLOW_OPEN")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            public_blobs: std::env::var("FE_REGISTRY_PUBLIC_BLOBS")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
        }
    }

    /// Default-deny: a registry with no bearer token must be opened on purpose.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.token.is_none() && !self.allow_open {
            anyhow::bail!(
                "FE_REGISTRY_TOKEN is not set; set a token, or FE_REGISTRY_ALLOW_OPEN=1 to serve open access"
            );
        }
        Ok(())
    }
}
