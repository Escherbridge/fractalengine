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
}

impl RegistryConfig {
    /// Read configuration from the environment, applying defaults.
    pub fn from_env() -> Self {
        Self {
            dir: std::env::var("FE_REGISTRY_DIR")
                .unwrap_or_else(|_| "/data/hexons".to_string())
                .into(),
            bind: std::env::var("FE_REGISTRY_BIND")
                .unwrap_or_else(|_| "0.0.0.0:8790".to_string()),
            token: std::env::var("FE_REGISTRY_TOKEN").ok().filter(|t| !t.is_empty()),
            readonly: std::env::var("FE_REGISTRY_READONLY")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
        }
    }
}
