//! Thin wrapper around [`iroh::Endpoint`] for P2P Mycelium networking (Phase D).
//!
//! The [`SyncEndpoint`] manages the iroh QUIC endpoint lifecycle:
//! construction from a deterministic secret key, relay connectivity, the
//! in-memory peer address book, and graceful shutdown. See
//! `fe-sync/src/AGENTS.md` §iroh-1.x.

use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::Result;
use iroh::address_lookup::MemoryLookup;

use crate::relay_config::RelayConfig;

/// Wraps an [`iroh::Endpoint`] with convenience accessors for the sync thread.
pub struct SyncEndpoint {
    endpoint: iroh::Endpoint,
    /// Out-of-band peer addresses (bootstrap peers), consulted when a peer is
    /// dialed by bare [`iroh::EndpointId`] — iroh 1.x's replacement for
    /// 0.35's `Endpoint::add_node_addr` (see AGENTS.md §gossip-bootstrap).
    address_book: MemoryLookup,
    /// Whether any relay is configured (`false` for [`RelayConfig::Disabled`]).
    relay_enabled: bool,
}

/// How long [`SyncEndpoint::node_addr`] waits for a home relay — iroh's own
/// net-report budget (`iroh::NET_REPORT_TIMEOUT`, in seconds).
const HOME_RELAY_WAIT: Duration = Duration::from_secs(iroh::NET_REPORT_TIMEOUT);

/// Guards the one-time startup warning about the rate-limited public n0 relays.
static DEFAULT_RELAY_WARNING: Once = Once::new();

/// Count of `SyncEndpoint`s bound in this process — the sim lab's
/// "no real network" assertion (F8/A19): a scenario run on a virtual
/// transport binds zero endpoints.
static BOUND_ENDPOINTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many iroh endpoints this process has bound (see
/// [`BOUND_ENDPOINTS`]). A virtual-transport (sim) sync thread never
/// constructs a `SyncEndpoint`, so a sim scenario asserts this stays at
/// its pre-run value.
pub fn bound_endpoint_count() -> u64 {
    BOUND_ENDPOINTS.load(std::sync::atomic::Ordering::SeqCst)
}

/// Warn — once per process — that iroh's default public n0 relays are
/// shared, rate-limited infrastructure (see AGENTS.md §relay-health).
/// Called whenever a [`SyncEndpoint`] binds against [`RelayConfig::Default`].
fn warn_default_relay_once() {
    DEFAULT_RELAY_WARNING.call_once(|| {
        tracing::warn!(
            env_var = crate::relay_config::RELAY_CONFIG_ENV_VAR,
            "Using iroh's default public n0 relay servers — shared infrastructure \
             that rate-limits relayed traffic. Configure a custom RelayConfig \
             (FE_SYNC_RELAY or the Settings relay; self-host iroh-relay 1.x) for \
             production. See fe-sync/src/AGENTS.md §relay-health."
        );
    });
}

/// QUIC transport config with explicit BBR (see AGENTS.md §congestion-control).
///
/// The builder starts from iroh's own multipath-tuned defaults (keep-alive,
/// path idle timeout, …), so only the congestion controller is overridden.
fn bbr_transport_config() -> iroh::endpoint::QuicTransportConfig {
    iroh::endpoint::QuicTransportConfig::builder()
        .congestion_controller_factory(Arc::new(noq_proto::congestion::Bbr3Config::default()))
        .build()
}

impl SyncEndpoint {
    /// Create a new endpoint bound per `relay_config`.
    ///
    /// The `secret_key` determines the node identity — the same 32-byte seed
    /// always produces the same [`iroh::EndpointId`]. `relay_config` is
    /// validated (via [`RelayConfig::to_relay_mode`]) *before* the bind
    /// attempt, so a misconfigured custom relay URL fails loudly here rather
    /// than queuing silently on first dial (see AGENTS.md §relay-health).
    /// Binding against [`RelayConfig::Default`] logs a one-time warning that
    /// the public n0 relays are rate-limited.
    ///
    /// Built on `presets::Minimal` (crypto provider only): no pkarr/DNS
    /// address publishing — peers are addressed out-of-band through the
    /// in-memory address book, exactly as on 0.35.
    pub async fn new(secret_key: iroh::SecretKey, relay_config: &RelayConfig) -> Result<Self> {
        let relay_mode = relay_config.to_relay_mode()?;
        if relay_config.is_default_infra() {
            warn_default_relay_once();
        }
        let address_book = MemoryLookup::new();
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .relay_mode(relay_mode)
            .transport_config(bbr_transport_config())
            .address_lookup(address_book.clone())
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("binding iroh endpoint: {e}"))?;
        BOUND_ENDPOINTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tracing::info!(
            congestion_controller = "bbr3",
            relay_config = ?relay_config,
            "iroh endpoint bound"
        );
        Ok(Self {
            endpoint,
            address_book,
            relay_enabled: !matches!(relay_config, RelayConfig::Disabled),
        })
    }

    /// The unique public-key identifier of this node.
    pub fn node_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// Our own dialable [`iroh::EndpointAddr`] (endpoint id + relay URL +
    /// direct addresses).
    ///
    /// With a relay configured this first waits — bounded by
    /// [`HOME_RELAY_WAIT`] (iroh's net-report timeout) — for a home relay connection so the
    /// address carries the relay URL; on timeout it returns the current
    /// snapshot (local addresses only) with a warn. With relays disabled it
    /// never waits (`Endpoint::online` would pend forever there).
    pub async fn node_addr(&self) -> iroh::EndpointAddr {
        if self.relay_enabled
            && tokio::time::timeout(HOME_RELAY_WAIT, self.endpoint.online())
                .await
                .is_err()
        {
            tracing::warn!(
                timeout = ?HOME_RELAY_WAIT,
                "no home relay connected yet — advertising local addresses only"
            );
        }
        self.endpoint.addr()
    }

    /// Register a peer's address in the endpoint's address book so a later
    /// dial by bare [`iroh::EndpointId`] (gossip join, docs sync) resolves.
    /// Addresses merge with any already known for that peer.
    pub fn add_peer_addr(&self, addr: iroh::EndpointAddr) {
        self.address_book.add_endpoint_info(addr);
    }

    /// Watcher over the connection status of the endpoint's home relays
    /// (SYNC-RELAYWATCH input — see AGENTS.md §relay-health).
    pub fn home_relay_status(
        &self,
    ) -> impl iroh::Watcher<Value = Vec<iroh::endpoint::RelayStatus>> + use<> {
        self.endpoint.home_relay_status()
    }

    /// Whether a relay is configured for this endpoint.
    pub fn relay_enabled(&self) -> bool {
        self.relay_enabled
    }

    /// A reference to the inner endpoint, for protocol integration.
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
                // Disabled relays: the address resolves immediately (never
                // waits on `online()`) and names our own id.
                let addr = ep.node_addr().await;
                assert_eq!(addr.id, expected_id);
                assert!(!ep.relay_enabled());
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
        // Type-level guarantee that the Bbr3Config factory unifies with
        // iroh's re-exported ControllerFactory (same noq-proto instance).
        let _ = bbr_transport_config();
    }

    #[test]
    fn deterministic_node_id_from_seed() {
        let a = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let b = iroh::SecretKey::from_bytes(&[7u8; 32]);
        assert_eq!(a.public(), b.public());
    }
}
