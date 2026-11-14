//! Thin wrapper around [`iroh::Endpoint`] for P2P Mycelium networking (Phase D).
//!
//! The [`SyncEndpoint`] manages the iroh QUIC endpoint lifecycle:
//! construction from a deterministic secret key, relay connectivity, the
//! in-memory peer address book, and graceful shutdown. See
//! `fe-sync/src/AGENTS.md` §iroh-1.x.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::path::Path;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Once, RwLock};
use std::time::Duration;

use anyhow::Result;
use iroh::address_lookup::MemoryLookup;

use crate::relay_config::{RelayAccessToken, RelayConfig};

/// Most relay URLs remembered per peer for re-filtering when the allowlist changes.
pub const MAX_REMEMBERED_RELAYS_PER_PEER: usize = 8;

/// File under the P2P data dir naming the UDP ports this node last bound (QA-0145).
pub const ENDPOINT_PORTS_FILE: &str = "endpoint-ports.json";

/// The UDP ports an endpoint bound, per address family; see `src/AGENTS.md` §stable-port.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoundPorts {
    pub v4: Option<u16>,
    pub v6: Option<u16>,
}

impl BoundPorts {
    fn of(sockets: &[SocketAddr]) -> Self {
        let port = |want_v4: bool| {
            sockets
                .iter()
                .find(|socket| socket.is_ipv4() == want_v4 && socket.port() != 0)
                .map(SocketAddr::port)
        };
        Self { v4: port(true), v6: port(false) }
    }

    fn load(p2p_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(p2p_dir.join(ENDPOINT_PORTS_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn store(self, p2p_dir: &Path) {
        let written = serde_json::to_string(&self)
            .map_err(anyhow::Error::from)
            .and_then(|json| {
                std::fs::create_dir_all(p2p_dir)?;
                std::fs::write(p2p_dir.join(ENDPOINT_PORTS_FILE), json)?;
                Ok(())
            });
        if let Err(error) = written {
            tracing::warn!("could not record the bound P2P ports; the next start binds fresh ones: {error:#}");
        }
    }
}

/// Wraps an [`iroh::Endpoint`] with convenience accessors for the sync thread.
pub struct SyncEndpoint {
    endpoint: iroh::Endpoint,
    /// Out-of-band peer addresses (bootstrap peers), consulted when a peer is
    /// dialed by bare [`iroh::EndpointId`] — iroh 1.x's replacement for
    /// 0.35's `Endpoint::add_node_addr` (see AGENTS.md §gossip-bootstrap).
    address_book: MemoryLookup,
    /// Whether any relay is configured (`false` for [`RelayConfig::Disabled`]).
    relay_enabled: bool,
    /// Whether a [`RelayAccessToken`] was attached at bind time (status
    /// surface only, A14 — the token value itself never lives on this
    /// struct; `to_relay_mode_with_token` consumes it before `bind()`).
    relay_token_configured: bool,
    /// Relay URLs a peer address may name; see `src/AGENTS.md` §relay-trust.
    relay_allowlist: Arc<RwLock<BTreeSet<iroh::RelayUrl>>>,
    /// Relay URLs each peer's addresses named (filtered or not), so an allowlist change re-applies to the book.
    peer_relays: Arc<Mutex<BTreeMap<iroh::EndpointId, BTreeSet<iroh::RelayUrl>>>>,
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
    ///
    /// Delegates to [`Self::with_relay_token`] with
    /// [`RelayAccessToken::from_env`], so production picks up an operator's
    /// relay token with **no edit to this call's callers** (A14;
    /// `fe-sync/src/sync_thread.rs:527` is this method's only production
    /// call site and is unchanged by W6-8).
    pub async fn new(secret_key: iroh::SecretKey, relay_config: &RelayConfig) -> Result<Self> {
        Self::with_relay_token(secret_key, relay_config, RelayAccessToken::from_env()).await
    }

    /// Like [`Self::new`], but with an explicit relay access token instead
    /// of reading [`RelayAccessToken::from_env`] — the test seam for a
    /// token-gated relay (A14).
    pub async fn with_relay_token(
        secret_key: iroh::SecretKey,
        relay_config: &RelayConfig,
        token: Option<RelayAccessToken>,
    ) -> Result<Self> {
        Self::bind(secret_key, relay_config, token, BoundPorts::default()).await
    }

    /// Like [`Self::new`], but re-binds the UDP ports recorded under `p2p_dir`
    /// so a restarted node keeps its address (QA-0145, `src/AGENTS.md` §stable-port).
    /// Taken ports fall back to fresh ones; the bound ports are recorded either way.
    pub async fn new_with_stable_ports(
        secret_key: iroh::SecretKey,
        relay_config: &RelayConfig,
        p2p_dir: &Path,
    ) -> Result<Self> {
        let token = RelayAccessToken::from_env();
        let previous = BoundPorts::load(p2p_dir).unwrap_or_default();
        let bound = if previous == BoundPorts::default() {
            Self::bind(secret_key, relay_config, token, previous).await
        } else {
            match Self::bind(secret_key.clone(), relay_config, token.clone(), previous).await {
                Ok(endpoint) => Ok(endpoint),
                Err(error) => {
                    tracing::warn!(?previous, "previous P2P ports are taken; binding fresh ones, so peers must re-learn this node's address: {error:#}");
                    Self::bind(secret_key, relay_config, token, BoundPorts::default()).await
                }
            }
        }?;
        BoundPorts::of(&bound.endpoint.bound_sockets()).store(p2p_dir);
        Ok(bound)
    }

    /// Bind on `ports` (`None` per family = a fresh port).
    async fn bind(
        secret_key: iroh::SecretKey,
        relay_config: &RelayConfig,
        token: Option<RelayAccessToken>,
        ports: BoundPorts,
    ) -> Result<Self> {
        let relay_mode = relay_config
            .to_relay_mode_with_token(token.as_ref())
            .map_err(|e| anyhow::anyhow!("resolving relay mode: {e}"))?;
        if relay_config.is_default_infra() {
            warn_default_relay_once();
        }
        let address_book = MemoryLookup::new();
        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .relay_mode(relay_mode)
            .transport_config(bbr_transport_config())
            .address_lookup(address_book.clone());
        if let Some(port) = ports.v4 {
            builder = builder
                .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
                .map_err(|e| anyhow::anyhow!("IPv4 bind address: {e}"))?;
        }
        if let Some(port) = ports.v6 {
            // Mirrors the default IPv6 socket, whose bind may fail.
            let optional = iroh::endpoint::BindOpts::default().set_is_required(false);
            builder = builder
                .bind_addr_with_opts(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0), optional)
                .map_err(|e| anyhow::anyhow!("IPv6 bind address: {e}"))?;
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("binding iroh endpoint: {e}"))?;
        BOUND_ENDPOINTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Only a boolean crosses into this log line — never the token
        // itself, and `relay_config` (the `?`-Debug'd value here and at
        // `sync_thread.rs:459`) never holds it either (A14/RL-1).
        tracing::info!(
            congestion_controller = "bbr3",
            relay_config = ?relay_config,
            relay_token_configured = token.is_some(),
            "iroh endpoint bound"
        );
        let relay_allowlist: BTreeSet<iroh::RelayUrl> = crate::relay_config::configured_urls(relay_config)
            .iter()
            .filter_map(|raw| raw.parse().ok())
            .collect();
        Ok(Self {
            endpoint,
            address_book,
            relay_allowlist: Arc::new(RwLock::new(relay_allowlist)),
            peer_relays: Arc::new(Mutex::new(BTreeMap::new())),
            relay_enabled: !matches!(relay_config, RelayConfig::Disabled),
            relay_token_configured: token.is_some(),
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
    /// Relay URLs outside the allowlist are stripped first (input filter, not an egress allowlist).
    pub fn add_peer_addr(&self, addr: iroh::EndpointAddr) {
        let filtered = {
            // Lock order: allowlist, then peer_relays, then the address book (same in `set_relay_allowlist`).
            let allowlist = self.relay_allowlist.read().unwrap_or_else(|poisoned| poisoned.into_inner());
            {
                let mut peer_relays = self.peer_relays.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let remembered = peer_relays.entry(addr.id).or_default();
                for transport in &addr.addrs {
                    if let iroh::TransportAddr::Relay(url) = transport {
                        if remembered.len() < MAX_REMEMBERED_RELAYS_PER_PEER {
                            remembered.insert(url.clone());
                        }
                    }
                }
            }
            let before = addr.addrs.len();
            let filtered = crate::relay_trust::filter_peer_addr(addr, &allowlist);
            let stripped = before - filtered.addrs.len();
            if stripped > 0 {
                tracing::debug!(peer = %filtered.id, stripped, "peer address relay URLs outside the allowlist were dropped");
            }
            filtered
        };
        self.address_book.add_endpoint_info(filtered);
    }

    /// Replace the set of relay URLs a peer address may name and re-apply it to every stored peer address.
    pub fn set_relay_allowlist(&self, urls: impl IntoIterator<Item = iroh::RelayUrl>) {
        let next: BTreeSet<iroh::RelayUrl> = urls.into_iter().collect();
        let mut allowlist = self.relay_allowlist.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *allowlist == next {
            return;
        }
        *allowlist = next;
        // The book merges addresses, so a relay that left the allowlist must be removed here and one that joined restored.
        let peer_relays = self.peer_relays.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for (peer, relays) in peer_relays.iter() {
            let Some(stored) = self.address_book.get_endpoint_info(*peer) else {
                continue;
            };
            let stored = stored.to_endpoint_addr();
            let mut addrs: BTreeSet<iroh::TransportAddr> = stored
                .addrs
                .iter()
                .filter(|transport| !matches!(transport, iroh::TransportAddr::Relay(_)))
                .cloned()
                .collect();
            addrs.extend(
                relays
                    .iter()
                    .filter(|url| allowlist.contains(*url))
                    .cloned()
                    .map(iroh::TransportAddr::Relay),
            );
            if addrs != stored.addrs {
                self.address_book.set_endpoint_info(iroh::EndpointAddr { id: *peer, addrs });
            }
        }
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

    /// Whether a [`RelayAccessToken`] was attached at bind time (status
    /// surface only — A14).
    pub fn relay_token_configured(&self) -> bool {
        self.relay_token_configured
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

                // AC-10: a peer address naming a non-allowlisted relay is stored without it.
                let stray_relay: iroh::RelayUrl = "https://stray-relay.example.test".parse().expect("relay url");
                let direct: SocketAddr = "192.0.2.7:4433".parse().expect("socket addr");
                let peer = iroh::SecretKey::from_bytes(&[44u8; 32]).public();
                ep.add_peer_addr(iroh::EndpointAddr::new(peer).with_relay_url(stray_relay.clone()).with_ip_addr(direct));
                let stored = ep.address_book.get_endpoint_info(peer).expect("peer stored");
                let stored = stored.to_endpoint_addr();
                assert!(stored.addrs.iter().all(|transport| !matches!(transport, iroh::TransportAddr::Relay(_))), "stray relay stripped");
                assert!(stored.ip_addrs().any(|ip| *ip == direct), "direct address kept");
                // Allowlisting it later restores the remembered relay; revoking it strips it again.
                let stored_relays = |ep: &SyncEndpoint| -> Vec<iroh::RelayUrl> {
                    let stored = ep.address_book.get_endpoint_info(peer).expect("peer stored").to_endpoint_addr();
                    stored.relay_urls().cloned().collect()
                };
                ep.set_relay_allowlist([stray_relay.clone()]);
                assert_eq!(stored_relays(&ep), vec![stray_relay.clone()], "allowlisted relay restored");
                ep.set_relay_allowlist(std::iter::empty());
                assert!(stored_relays(&ep).is_empty(), "revoked relay stripped from the stored address");
                let stored = ep.address_book.get_endpoint_info(peer).expect("peer stored").to_endpoint_addr();
                assert!(stored.ip_addrs().any(|ip| *ip == direct), "direct address survives the re-filter");
                ep.shutdown().await;

                // QA-0145: a restarted node re-binds the ports it recorded, so the
                // address peers cached for its id stays live across the restart.
                let p2p_dir = tempfile::tempdir().expect("temp p2p dir");
                let first = SyncEndpoint::new_with_stable_ports(iroh::SecretKey::from_bytes(&[42u8; 32]), &RelayConfig::Disabled, p2p_dir.path())
                    .await
                    .expect("first stable bind");
                let first_ports = BoundPorts::of(&first.inner().bound_sockets());
                assert!(first_ports.v4.is_some(), "an IPv4 port is bound and recorded");
                assert_eq!(BoundPorts::load(p2p_dir.path()), Some(first_ports));
                first.shutdown().await;
                let restarted = SyncEndpoint::new_with_stable_ports(iroh::SecretKey::from_bytes(&[42u8; 32]), &RelayConfig::Disabled, p2p_dir.path())
                    .await
                    .expect("restart bind");
                assert_eq!(BoundPorts::of(&restarted.inner().bound_sockets()).v4, first_ports.v4, "the restart keeps its IPv4 port");

                // A taken port never blocks the start: the next bind falls back to fresh ports.
                let rival = SyncEndpoint::new_with_stable_ports(iroh::SecretKey::from_bytes(&[43u8; 32]), &RelayConfig::Disabled, p2p_dir.path())
                    .await
                    .expect("a taken port falls back to a fresh one");
                assert_ne!(BoundPorts::of(&rival.inner().bound_sockets()).v4, first_ports.v4);
                restarted.shutdown().await;
                rival.shutdown().await;
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
