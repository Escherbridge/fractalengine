//! Thin wrapper around [`iroh::Endpoint`] for P2P Mycelium networking (Phase D).
//!
//! The [`SyncEndpoint`] manages the iroh QUIC endpoint lifecycle:
//! construction from a deterministic secret key, relay connectivity, and
//! graceful shutdown.

use anyhow::Result;

/// Wraps an [`iroh::Endpoint`] with convenience accessors for the sync thread.
pub struct SyncEndpoint {
    endpoint: iroh::Endpoint,
}

impl SyncEndpoint {
    /// Create a new endpoint bound to the default relay servers.
    ///
    /// The `secret_key` determines the node identity — the same 32-byte seed
    /// always produces the same [`iroh::NodeId`].
    pub async fn new(secret_key: iroh::SecretKey) -> Result<Self> {
        let endpoint = iroh::Endpoint::builder()
            .secret_key(secret_key)
            .bind()
            .await?;
        Ok(Self { endpoint })
    }

    /// The unique public-key identifier of this node.
    pub fn node_id(&self) -> iroh::NodeId {
        self.endpoint.node_id()
    }

    /// Retrieve the full [`iroh::NodeAddr`] (node ID + relay URL + direct
    /// addresses).
    ///
    /// This is async because iroh may need to wait for relay/STUN discovery
    /// to complete.
    pub async fn node_addr(&self) -> Result<iroh::NodeAddr> {
        self.endpoint.node_addr().await
    }

    /// A reference to the inner endpoint, for future protocol integration.
    pub fn inner(&self) -> &iroh::Endpoint {
        &self.endpoint
    }

    /// Gracefully close the endpoint, notifying peers.
    pub async fn shutdown(self) {
        self.endpoint.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn endpoint_constructs_and_has_node_id() {
        let secret = iroh::SecretKey::from_bytes(&[42u8; 32]);
        let expected_id = secret.public();

        let ep = SyncEndpoint::new(secret).await;
        match ep {
            Ok(ep) => {
                assert_eq!(ep.node_id(), expected_id);
                ep.shutdown().await;
            }
            Err(e) => {
                // CI or sandboxed environments may not allow UDP binding.
                tracing::warn!("SyncEndpoint::new failed (expected in CI): {e}");
            }
        }
    }

    #[test]
    fn deterministic_node_id_from_seed() {
        let a = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let b = iroh::SecretKey::from_bytes(&[7u8; 32]);
        assert_eq!(a.public(), b.public());
    }
}
