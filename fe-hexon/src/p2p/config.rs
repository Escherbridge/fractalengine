use serde::{Deserialize, Serialize};
use std::env;

/// Configuration for P2P blob transfer behaviour.
///
/// Controls seeding, hosting, concurrency limits, and bandwidth throttling.
/// Values can be overridden via environment variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// Whether this node re-seeds crates it has downloaded.
    /// Default: true for relays, false for mobile.
    /// Env: FE_SEED_CRATES
    pub seed_crates: bool,

    /// Whether this node permanently hosts published crates (relay behaviour).
    /// Env: FE_CRATE_HOST
    pub crate_host: bool,

    /// Maximum number of concurrent blob downloads.
    /// Env: FE_MAX_CONCURRENT_DOWNLOADS
    pub max_concurrent_downloads: usize,

    /// Optional bandwidth cap in bytes/sec. None means unlimited.
    /// Env: FE_MAX_DOWNLOAD_RATE
    pub max_download_rate_bytes_per_sec: Option<u64>,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            seed_crates: true,
            crate_host: false,
            max_concurrent_downloads: 3,
            max_download_rate_bytes_per_sec: None,
        }
    }
}

impl P2pConfig {
    /// Build configuration from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let seed_crates = env::var("FE_SEED_CRATES")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);

        let crate_host = env::var("FE_CRATE_HOST")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        let max_concurrent_downloads = env::var("FE_MAX_CONCURRENT_DOWNLOADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        let max_download_rate_bytes_per_sec = env::var("FE_MAX_DOWNLOAD_RATE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());

        Self {
            seed_crates,
            crate_host,
            max_concurrent_downloads,
            max_download_rate_bytes_per_sec,
        }
    }
}

/// Priority tier for selecting which peer to fetch from first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PeerPriority {
    /// Local network peer — lowest latency, highest priority.
    Lan = 0,
    /// Known low-latency peer (e.g. same region relay).
    LowLatency = 1,
    /// Remote peer — highest latency, lowest priority.
    Remote = 2,
}

/// A candidate peer that may be able to serve a blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerCandidate {
    /// The DID of the peer node.
    pub peer_did: String,
    /// Priority tier for ordering fetch attempts.
    pub priority: PeerPriority,
    /// Milliseconds since this peer was last seen online (None if unknown).
    pub last_seen_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = P2pConfig::default();
        assert!(cfg.seed_crates);
        assert!(!cfg.crate_host);
        assert_eq!(cfg.max_concurrent_downloads, 3);
        assert_eq!(cfg.max_download_rate_bytes_per_sec, None);
    }

    #[test]
    fn test_from_env_defaults() {
        // Clear any env vars that might interfere
        env::remove_var("FE_SEED_CRATES");
        env::remove_var("FE_CRATE_HOST");
        env::remove_var("FE_MAX_CONCURRENT_DOWNLOADS");
        env::remove_var("FE_MAX_DOWNLOAD_RATE");

        let cfg = P2pConfig::from_env();
        assert!(cfg.seed_crates);
        assert!(!cfg.crate_host);
        assert_eq!(cfg.max_concurrent_downloads, 3);
        assert_eq!(cfg.max_download_rate_bytes_per_sec, None);
    }

    #[test]
    fn test_from_env_overrides() {
        env::set_var("FE_SEED_CRATES", "false");
        env::set_var("FE_CRATE_HOST", "true");
        env::set_var("FE_MAX_CONCURRENT_DOWNLOADS", "8");
        env::set_var("FE_MAX_DOWNLOAD_RATE", "1048576");

        let cfg = P2pConfig::from_env();
        assert!(!cfg.seed_crates);
        assert!(cfg.crate_host);
        assert_eq!(cfg.max_concurrent_downloads, 8);
        assert_eq!(cfg.max_download_rate_bytes_per_sec, Some(1_048_576));

        // Clean up
        env::remove_var("FE_SEED_CRATES");
        env::remove_var("FE_CRATE_HOST");
        env::remove_var("FE_MAX_CONCURRENT_DOWNLOADS");
        env::remove_var("FE_MAX_DOWNLOAD_RATE");
    }

    #[test]
    fn test_peer_priority_ordering() {
        assert!(PeerPriority::Lan < PeerPriority::LowLatency);
        assert!(PeerPriority::LowLatency < PeerPriority::Remote);
    }
}
