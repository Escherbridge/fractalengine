//! Thin wrapper around [`iroh::Endpoint`] for P2P Mycelium networking (Phase D).
//!
//! The [`SyncEndpoint`] manages the iroh QUIC endpoint lifecycle:
//! construction from a deterministic secret key, relay connectivity, and
//! graceful shutdown.

use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::Result;

use crate::relay_config::RelayConfig;

/// Wraps an [`iroh::Endpoint`] with convenience accessors for the sync thread.
pub struct SyncEndpoint {
    endpoint: iroh::Endpoint,
}

/// Guards the one-time startup warning about iroh 0.35's EOL-bound default relays.
static DEFAULT_RELAY_EOL_WARNING: Once = Once::new();

/// Warn — once per process — that iroh's default n0-hosted relay servers EOL
/// 2026-12-31 (see AGENTS.md §relay-health, decision D-77). Called whenever a
/// [`SyncEndpoint`] binds against [`RelayConfig::Default`].
fn warn_default_relay_eol_once() {
    DEFAULT_RELAY_EOL_WARNING.call_once(|| {
        tracing::warn!(
            eol_date = "2026-12-31",
            env_var = crate::relay_config::RELAY_CONFIG_ENV_VAR,
            "Using iroh's default n0-hosted relay servers — this infrastructure \
             EOLs 2026-12-31. Configure a custom RelayConfig before then, or P2P \
             connectivity for offline/behind-NAT peers will quietly degrade. \
             See fe-sync/src/AGENTS.md §relay-health."
        );
    });
}

/// QUIC transport config with explicit BBR (see AGENTS.md §congestion-control).
fn bbr_transport_config() -> iroh::endpoint::TransportConfig {
    let mut transport = iroh::endpoint::TransportConfig::default();
    // Re-apply iroh's builder default (replaced wholesale by a custom config).
    transport.keep_alive_interval(Some(Duration::from_secs(1)));
    transport.congestion_controller_factory(Arc::new(
        iroh_quinn_proto::congestion::BbrConfig::default(),
    ));
    transport
}

impl SyncEndpoint {
    /// Create a new endpoint bound per `relay_config`.
    ///
    /// The `secret_key` determines the node identity — the same 32-byte seed
    /// always produces the same [`iroh::NodeId`]. `relay_config` is validated
    /// (via [`RelayConfig::to_relay_mode`]) *before* the bind attempt, so a
    /// misconfigured custom relay URL fails loudly here rather than queuing
    /// silently on first dial (see AGENTS.md §relay-health). Binding against
    /// [`RelayConfig::Default`] logs a one-time EOL warning for iroh 0.35's
    /// n0-hosted relay infra (2026-12-31).
    pub async fn new(secret_key: iroh::SecretKey, relay_config: &RelayConfig) -> Result<Self> {
        let relay_mode = relay_config.to_relay_mode()?;
        if relay_config.is_default_infra() {
            warn_default_relay_eol_once();
        }
        let endpoint = iroh::Endpoint::builder()
            .secret_key(secret_key)
            .relay_mode(relay_mode)
            .transport_config(bbr_transport_config())
            .bind()
            .await?;
        tracing::info!(
            congestion_controller = "bbr",
            relay_config = ?relay_config,
            "iroh endpoint bound"
        );
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

        // RelayConfig::Disabled keeps this test hermetic (no relay contact).
        let ep = SyncEndpoint::new(secret, &RelayConfig::Disabled).await;
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

    #[tokio::test]
    async fn endpoint_new_rejects_invalid_custom_relay_url() {
        let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        // Constructed directly (bypassing `RelayConfig::parse`) — new() must
        // still validate before attempting to bind (loud-fail at construction).
        let bad_config = RelayConfig::Custom(vec!["not a url".to_string()]);

        let result = SyncEndpoint::new(secret, &bad_config).await;
        assert!(
            result.is_err(),
            "invalid custom relay URL should fail before binding"
        );
    }

    #[test]
    fn bbr_transport_config_constructs() {
        // Type-level guarantee that the BbrConfig factory unifies with iroh's
        // re-exported ControllerFactory (same iroh-quinn-proto instance).
        let _ = bbr_transport_config();
    }

    #[test]
    fn deterministic_node_id_from_seed() {
        let a = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let b = iroh::SecretKey::from_bytes(&[7u8; 32]);
        assert_eq!(a.public(), b.public());
    }
}
