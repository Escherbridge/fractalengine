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
    /// Optional bearer token gating only `publish` (`FE_REGISTRY_PUBLISH_TOKEN`); unset = the read token gates it.
    pub publish_token: Option<String>,
    /// Concurrent archive reads (`FE_REGISTRY_ARCHIVE_CONCURRENCY`, default 4).
    pub archive_concurrency: usize,
    /// Largest accepted publish body in bytes (`FE_REGISTRY_MAX_PUBLISH_BYTES`, default 64 MiB).
    pub max_publish_bytes: usize,
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
            bind: resolve_bind(
                std::env::var("FE_REGISTRY_BIND").ok(),
                std::env::var("PORT").ok(),
            ),
            token: token_env("FE_REGISTRY_TOKEN"),
            publish_token: token_env("FE_REGISTRY_PUBLISH_TOKEN"),
            archive_concurrency: number_env(
                "FE_REGISTRY_ARCHIVE_CONCURRENCY",
                crate::guard::DEFAULT_ARCHIVE_CONCURRENCY,
            )
            .max(1),
            max_publish_bytes: number_env("FE_REGISTRY_MAX_PUBLISH_BYTES", DEFAULT_MAX_PUBLISH_BYTES),
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
        for (name, token) in [
            ("FE_REGISTRY_TOKEN", &self.token),
            ("FE_REGISTRY_PUBLISH_TOKEN", &self.publish_token),
        ] {
            if token.as_ref().is_some_and(|t| t.len() < MIN_TOKEN_LEN) {
                anyhow::bail!("{name} is shorter than {MIN_TOKEN_LEN} characters");
            }
        }
        if self.token.is_none() && !self.allow_open {
            anyhow::bail!(
                "FE_REGISTRY_TOKEN is not set; set a token, or FE_REGISTRY_ALLOW_OPEN=1 to serve open access"
            );
        }
        if self.token.is_none() && !self.readonly && self.publish_token.is_none() {
            anyhow::bail!(
                "open reads need a writer guard: set FE_REGISTRY_PUBLISH_TOKEN, or FE_REGISTRY_READONLY=1"
            );
        }
        Ok(())
    }
}

/// Default publish cap: the desktop client refuses larger packages.
pub const DEFAULT_MAX_PUBLISH_BYTES: usize = 64 << 20;
/// Shortest bearer token the service will start with.
pub const MIN_TOKEN_LEN: usize = 32;

/// Trimmed token from the environment; empty = unset.
fn token_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Numeric env var, falling back to `default` when unset or unparsable.
fn number_env<T: std::str::FromStr>(name: &str, default: T) -> T {
    match std::env::var(name) {
        Ok(raw) => raw.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(variable = name, "not a number; using the default");
            default
        }),
        Err(_) => default,
    }
}

/// `FE_REGISTRY_BIND` wins, then `0.0.0.0:$PORT`, then the default `0.0.0.0:8790`.
fn resolve_bind(bind: Option<String>, port: Option<String>) -> String {
    match (bind.filter(|b| !b.is_empty()), port.filter(|p| !p.is_empty())) {
        (Some(bind), _) => bind,
        (None, Some(port)) => format!("0.0.0.0:{port}"),
        (None, None) => "0.0.0.0:8790".to_string(),
    }
}
