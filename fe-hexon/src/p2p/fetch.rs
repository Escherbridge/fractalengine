use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Semaphore;

use crate::manifest::HexonManifest;
use crate::signature::verify_manifest;

use super::config::{P2pConfig, PeerCandidate};

/// Errors that can occur during blob fetch operations.
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("manifest signature is invalid")]
    SignatureInvalid,

    #[error("no peers available to serve the requested blob")]
    NoPeersAvailable,

    #[error("failed to fetch manifest: {0}")]
    ManifestFetchFailed(String),

    #[error("failed to fetch blob: {0}")]
    BlobFetchFailed(String),

    #[error("operation timed out")]
    Timeout,

    #[error("all candidate peers failed to serve the blob")]
    AllPeersFailed,
}

/// Controls how blobs are fetched from the P2P network.
///
/// Manages concurrency limits via a semaphore and exposes policy
/// decisions (seeding, hosting) based on `P2pConfig`.
#[derive(Debug, Clone)]
pub struct FetchStrategy {
    config: P2pConfig,
    download_semaphore: Arc<Semaphore>,
}

impl FetchStrategy {
    /// Create a new fetch strategy from configuration.
    pub fn new(config: P2pConfig) -> Self {
        let download_semaphore = Arc::new(Semaphore::new(config.max_concurrent_downloads));
        Self {
            config,
            download_semaphore,
        }
    }

    /// Sort peer candidates by priority (LAN first, then LowLatency, then Remote).
    pub fn prioritize_peers(candidates: &mut [PeerCandidate]) {
        candidates.sort_by_key(|c| c.priority);
    }

    /// Whether this node should re-seed downloaded crates to other peers.
    pub fn should_seed(&self) -> bool {
        self.config.seed_crates
    }

    /// Whether this node should permanently host published crates.
    pub fn should_host(&self) -> bool {
        self.config.crate_host
    }

    /// Acquire a download slot, respecting concurrency limits.
    ///
    /// Returns a permit that must be held for the duration of the download.
    /// Times out after 30 seconds if no slot becomes available.
    pub async fn acquire_download_slot(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, FetchError> {
        let sem = self.download_semaphore.clone();
        tokio::time::timeout(Duration::from_secs(30), sem.acquire_owned())
            .await
            .map_err(|_| FetchError::Timeout)?
            .map_err(|_| FetchError::NoPeersAvailable)
    }

    /// Access the underlying config.
    pub fn config(&self) -> &P2pConfig {
        &self.config
    }
}

/// Result of successfully fetching and parsing a manifest.
#[derive(Debug, Clone)]
pub struct FetchManifestResult {
    /// The parsed manifest.
    pub manifest: HexonManifest,
    /// Raw manifest bytes (for hashing/storage).
    pub manifest_bytes: Vec<u8>,
    /// Whether the cryptographic signature verified against the publisher DID.
    pub signature_valid: bool,
}

/// Parse and verify a fetched manifest blob.
///
/// Deserializes the JSON, then checks the `signature` field against
/// the provided `publisher_did`. Returns `FetchError::SignatureInvalid`
/// if the signature does not verify.
pub fn verify_fetched_manifest(
    manifest_bytes: &[u8],
    publisher_did: &str,
) -> Result<FetchManifestResult, FetchError> {
    let manifest: HexonManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|e| FetchError::ManifestFetchFailed(format!("JSON parse error: {e}")))?;

    // Verify the signature field against the manifest content.
    // We need to verify against the bytes WITHOUT the signature field to avoid
    // circular dependency. However, the current signature scheme signs the full
    // manifest JSON as provided during publishing. We verify using the raw bytes
    // and the signature extracted from the parsed manifest.
    let signature_valid = verify_manifest(manifest_bytes, &manifest.signature, publisher_did)
        .map_err(|e| FetchError::ManifestFetchFailed(format!("Signature check error: {e}")))?;

    if !signature_valid {
        return Err(FetchError::SignatureInvalid);
    }

    Ok(FetchManifestResult {
        manifest,
        manifest_bytes: manifest_bytes.to_vec(),
        signature_valid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::config::PeerPriority;

    #[test]
    fn test_prioritize_peers_ordering() {
        let mut candidates = vec![
            PeerCandidate {
                peer_did: "did:key:remote1".to_string(),
                priority: PeerPriority::Remote,
                last_seen_ms: Some(5000),
            },
            PeerCandidate {
                peer_did: "did:key:lan1".to_string(),
                priority: PeerPriority::Lan,
                last_seen_ms: Some(100),
            },
            PeerCandidate {
                peer_did: "did:key:low1".to_string(),
                priority: PeerPriority::LowLatency,
                last_seen_ms: None,
            },
            PeerCandidate {
                peer_did: "did:key:lan2".to_string(),
                priority: PeerPriority::Lan,
                last_seen_ms: Some(200),
            },
        ];

        FetchStrategy::prioritize_peers(&mut candidates);

        assert_eq!(candidates[0].priority, PeerPriority::Lan);
        assert_eq!(candidates[1].priority, PeerPriority::Lan);
        assert_eq!(candidates[2].priority, PeerPriority::LowLatency);
        assert_eq!(candidates[3].priority, PeerPriority::Remote);
    }

    #[test]
    fn test_fetch_strategy_policy() {
        let config = P2pConfig {
            seed_crates: true,
            crate_host: false,
            max_concurrent_downloads: 5,
            max_download_rate_bytes_per_sec: None,
        };
        let strategy = FetchStrategy::new(config);

        assert!(strategy.should_seed());
        assert!(!strategy.should_host());
    }

    #[test]
    fn test_verify_fetched_manifest_invalid_json() {
        let result = verify_fetched_manifest(b"not json at all", "did:key:z123");
        assert!(result.is_err());
        match result.unwrap_err() {
            FetchError::ManifestFetchFailed(msg) => {
                assert!(msg.contains("JSON parse error"));
            }
            other => panic!("expected ManifestFetchFailed, got: {other:?}"),
        }
    }

    #[test]
    fn test_verify_fetched_manifest_bad_signature() {
        // Build a valid JSON manifest but with a bogus signature and valid did:key format
        let manifest_json = serde_json::json!({
            "schema_version": 1,
            "crate_id": "test-crate",
            "publisher_did": "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
            "publisher_name": "Test",
            "version": "1.0.0",
            "build_id": "abc123",
            "name": "Test Hexon",
            "description": "test",
            "hexon_type": "Model",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "min_engine_version": "0.8.0",
            "signature": "aW52YWxpZHNpZ25hdHVyZQ=="
        });

        let bytes = serde_json::to_vec(&manifest_json).unwrap();
        let result = verify_fetched_manifest(
            &bytes,
            "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
        );

        // Should fail — signature is garbage
        assert!(matches!(
            result,
            Err(FetchError::SignatureInvalid) | Err(FetchError::ManifestFetchFailed(_))
        ));
    }

    #[tokio::test]
    async fn test_acquire_download_slot_success() {
        let config = P2pConfig {
            seed_crates: false,
            crate_host: false,
            max_concurrent_downloads: 2,
            max_download_rate_bytes_per_sec: None,
        };
        let strategy = FetchStrategy::new(config);

        let permit1 = strategy.acquire_download_slot().await;
        assert!(permit1.is_ok());

        let permit2 = strategy.acquire_download_slot().await;
        assert!(permit2.is_ok());

        // Both slots consumed — drop one to free
        drop(permit1);
        let permit3 = strategy.acquire_download_slot().await;
        assert!(permit3.is_ok());
    }
}
